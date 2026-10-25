use super::{capture, dto::CaptureInput, models::*, store};
use crate::{
    common::{
        canonical,
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared::{self, Actor},
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

pub async fn create(
    database: &Database,
    actor: Actor,
    input: CaptureInput,
) -> Result<MetaView, AppError> {
    let selection = capture::prepare(&actor, &input)?;
    let id = ObjectId::new();
    let record = transaction::run_snapshot(
        database,
        (database.clone(), actor, input, selection),
        |session, (database, actor, input, selection)| {
            async move {
                shared::lock(database, "versions", session).await?;
                if database
                    .collection::<Document>("versions")
                    .find_one(doc! {"codeKey":input.code.to_ascii_lowercase()})
                    .session(&mut *session)
                    .await?
                    .is_some()
                {
                    return Err(fail(Failure::Conflict("版本编号已存在，历史编号不能复用")));
                }
                let now = DateTime::now();
                let (resources, counts) = capture::resources(database, selection, session).await?;
                let bundle = Bundle {
                    format: "tri-admin/config-bundle".into(),
                    schema_version: 1,
                    release: Release {
                        name: input.name.clone(),
                        code: input.code.clone(),
                        description: input.description.clone(),
                        captured_at: time(now)
                            .map_err(|_| fail(Failure::Validation("时间无法编码")))?,
                    },
                    resources,
                    required_apis: selection.apis.clone(),
                };
                let json = canonical::json(&bundle)
                    .map_err(|_| fail(Failure::Validation("配置包无法编码")))?;
                if json.len() > MAX_PACKAGE_BYTES {
                    return Err(fail(Failure::PayloadTooLarge));
                }
                let checksum = canonical::checksum(json.as_bytes());
                let record = store::insert(
                    database,
                    actor,
                    store::Package {
                        id,
                        bundle: &bundle,
                        json: &json,
                        checksum: &checksum,
                        counts: &counts,
                        reads: &selection.read_permissions,
                        origin: "capture",
                    },
                    session,
                )
                .await?;
                shared::audit(database, actor, "versions.create", id, session).await?;
                Ok(record)
            }
            .boxed()
        },
    )
    .await?;
    record.view()
}
pub async fn list(database: &Database, input: PageInput) -> Result<Page<MetaView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "name", "code"])?;
    let sort: Document = sort
        .into_iter()
        .map(|(key, value)| (if key == "code" { "codeKey".into() } else { key }, value))
        .collect();
    let mut filter = doc! {"deletedAt":null};
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mut prefix = "^".to_owned();
        for character in search.to_ascii_lowercase().chars() {
            if ".+*?^${}()|[]\\".contains(character) {
                prefix.push('\\');
            }
            prefix.push(character);
        }
        filter.insert("codeKey", doc! {"$regex":prefix});
    }
    let total = database
        .collection::<Document>("versions")
        .count_documents(filter.clone())
        .await?;
    let data = database
        .collection::<MetaRecord>("versions")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .map(MetaRecord::view)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(data, &input, total))
}
pub async fn get(database: &Database, id: ObjectId) -> Result<MetaView, AppError> {
    database
        .collection::<MetaRecord>("versions")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
pub async fn package(
    database: &Database,
    actor: &Actor,
    id: ObjectId,
) -> Result<StoredPackage, AppError> {
    let meta = database
        .collection::<MetaRecord>("versions")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?;
    if meta.read_permissions.iter().any(|code| !actor.can(code)) {
        return Err(AppError::Forbidden);
    }
    let payload = database
        .collection::<PackageRecord>("version_packages")
        .find_one(doc! {"_id":id})
        .await?
        .ok_or_else(|| AppError::Conflict("配置快照缺失".into()))?;
    let package = StoredPackage {
        meta,
        package_json: payload.package_json,
    };
    if package.package_json.len() > MAX_PACKAGE_BYTES
        || package.package_json.len() as i64 != package.meta.byte_length
        || canonical::checksum(package.package_json.as_bytes()) != package.meta.checksum
    {
        return Err(AppError::Conflict("配置快照校验失败".into()));
    }
    Ok(package)
}
pub async fn delete(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    if !(1..i64::MAX).contains(&version) {
        return Err(AppError::Validation("版本号无效".into()));
    }
    transaction::run(
        database,
        (database.clone(), actor),
        |session, (database, actor)| {
            async move {
                shared::lock(database, "versions", session).await?;
                let record = database
                    .collection::<MetaRecord>("versions")
                    .find_one(doc! {"_id":id,"deletedAt":null})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if record.version != version {
                    return Err(fail(Failure::Conflict("数据已被修改，请刷新后重试")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("versions")
                    .update_one(
                        doc! {"_id":id},
                        doc! {"$set":{"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}},
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "versions.delete", id, session).await
            }
            .boxed()
        },
    )
    .await
}
