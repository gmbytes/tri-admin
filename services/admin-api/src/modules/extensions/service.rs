use super::{
    dto::{self, ConstraintInput, Install, Revise, Source, Toggle},
    models::*,
};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
    },
    modules::{
        auth::AuthContext,
        rbac::{
            owned::{self, Resource},
            shared::{self, Actor},
        },
    },
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{self, Binary, DateTime, Document, doc, oid::ObjectId, spec::BinarySubtype},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tri_extension_package::package::{self, Package};
pub(super) fn policy() -> Resource {
    Resource {
        schema: "extensions",
        version: crate::storage::extensions_schema::VERSION,
        read: "skills.read",
        write: "skills.write",
    }
}
fn fingerprint<T: Serialize>(
    operation: &str,
    id: Option<ObjectId>,
    input: &T,
) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(&(operation, id.map(|v| v.to_hex()), input))
        .map_err(|_| AppError::Internal)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
async fn build(source: Source, base: Option<Package>) -> Result<Package, AppError> {
    tokio::task::spawn_blocking(move || source.build(base))
        .await
        .map_err(|_| AppError::Internal)?
}
pub(super) async fn row_tx(
    database: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    id: ObjectId,
    active: bool,
) -> mongodb::error::Result<Extension> {
    let mut filter = doc! {"_id":id};
    if active {
        filter.insert("deletedAt", bson::Bson::Null);
    }
    let row = database
        .collection::<Extension>("extensions")
        .find_one(filter)
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    owned::owns(database, session, actor, row.owner_id).await?;
    Ok(row)
}
async fn receipt_tx(
    database: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    request: &str,
    expected: Option<&str>,
) -> mongodb::error::Result<Option<Extension>> {
    let row = database
        .collection::<Receipt>("extension_requests")
        .find_one(doc! {"actorId":actor.id,"requestId":request})
        .session(&mut *session)
        .await?;
    if let Some(row) = row {
        owned::owns(database, session, actor, row.result.owner_id).await?;
        if expected.is_some_and(|v| v != row.fingerprint) {
            return Err(fail(Failure::Conflict("请求标识已用于不同扩展变更")));
        }
        Ok(Some(row.result))
    } else {
        Ok(None)
    }
}
async fn existing(
    database: &Database,
    context: &AuthContext,
    request: &str,
    expected: Option<&str>,
    write: bool,
) -> Result<Option<Extension>, AppError> {
    dto::request_id(request)?;
    transaction::run_snapshot(
        database,
        (
            database.clone(),
            context.clone(),
            request.to_owned(),
            expected.map(String::from),
        ),
        |session, (database, context, request, expected)| {
            async move {
                let actor = owned::fresh(database, session, context, policy(), false).await?;
                if write && !actor.can("skills.write") {
                    return Err(fail(Failure::Forbidden));
                }
                receipt_tx(database, session, &actor, request, expected.as_deref()).await
            }
            .boxed()
        },
    )
    .await
}
pub async fn by_request(
    database: &Database,
    context: &AuthContext,
    request: &str,
) -> Result<ExtensionView, AppError> {
    existing(database, context, request, None, false)
        .await?
        .ok_or(AppError::NotFound)?
        .view()
}
async fn save_revision(
    database: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    row: &Extension,
    request: &str,
    package: &Package,
) -> mongodb::error::Result<()> {
    let revision = Revision {
        meta: RevisionMeta {
            id: row.revision_id,
            extension_id: row.id,
            owner_id: row.owner_id,
            actor_id: actor.id,
            request_id: request.into(),
            number: row.revision,
            package: package.info(),
            created_at: row.updated_at,
        },
        archive_bytes: Binary {
            subtype: BinarySubtype::Generic,
            bytes: package.archive.clone(),
        },
    };
    database
        .collection::<Revision>("extension_revisions")
        .insert_one(revision)
        .session(session)
        .await?;
    Ok(())
}
async fn save_receipt(
    database: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    row: &Extension,
    request: &str,
    fingerprint: &str,
) -> mongodb::error::Result<()> {
    database
        .collection::<Receipt>("extension_requests")
        .insert_one(Receipt {
            actor_id: actor.id,
            request_id: request.into(),
            fingerprint: fingerprint.into(),
            result: row.clone(),
        })
        .session(session)
        .await?;
    Ok(())
}
pub async fn install(
    database: &Database,
    context: &AuthContext,
    input: Install,
) -> Result<(ExtensionView, bool), AppError> {
    dto::request_id(&input.request_id)?;
    let fingerprint = fingerprint("install", None, &input)?;
    if let Some(row) = existing(
        database,
        context,
        &input.request_id,
        Some(&fingerprint),
        true,
    )
    .await?
    {
        return Ok((row.view()?, false));
    }
    let package = build(input.source, None).await?;
    let id = ObjectId::new();
    let revision_id = ObjectId::new();
    let now = DateTime::now();
    let (row, created) = transaction::run(
        database,
        (
            database.clone(),
            context.clone(),
            input.request_id,
            fingerprint,
            package,
        ),
        |session, (database, context, request, fingerprint, package)| {
            async move {
                let actor = owned::fresh(database, session, context, policy(), true).await?;
                if let Some(row) =
                    receipt_tx(database, session, &actor, request, Some(fingerprint)).await?
                {
                    return Ok((row, false));
                }
                let row = Extension {
                    id,
                    owner_id: actor.id,
                    name: package.name.clone(),
                    kind: package.kind,
                    enabled: false,
                    version: 1,
                    revision: 1,
                    revision_id,
                    package: package.info(),
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                database
                    .collection::<Extension>("extensions")
                    .insert_one(&row)
                    .session(&mut *session)
                    .await?;
                save_revision(database, session, &actor, &row, request, package).await?;
                save_receipt(database, session, &actor, &row, request, fingerprint).await?;
                shared::audit(database, &actor, "skills.install", id, session).await?;
                Ok((row, true))
            }
            .boxed()
        },
    )
    .await?;
    Ok((row.view()?, created))
}
pub async fn get(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
) -> Result<ExtensionView, AppError> {
    get_record(database, context, id, true).await?.view()
}
async fn get_record(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    active: bool,
) -> Result<Extension, AppError> {
    transaction::run_snapshot(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                let actor = owned::fresh(database, session, context, policy(), false).await?;
                row_tx(database, session, &actor, id, active).await
            }
            .boxed()
        },
    )
    .await
}
async fn revision_record(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    revision: ObjectId,
) -> Result<Revision, AppError> {
    transaction::run_snapshot(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                let actor = owned::fresh(database, session, context, policy(), false).await?;
                let row = row_tx(database, session, &actor, id, false).await?;
                database
                    .collection::<Revision>("extension_revisions")
                    .find_one(doc! {"_id":revision,"extensionId":id,"ownerId":row.owner_id})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))
            }
            .boxed()
        },
    )
    .await
}
pub async fn revision(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    revision: ObjectId,
) -> Result<RevisionView, AppError> {
    revision_record(database, context, id, revision)
        .await?
        .meta
        .view()
}
pub async fn load_package(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    revision: ObjectId,
) -> Result<(RevisionMeta, Package), AppError> {
    let record = revision_record(database, context, id, revision).await?;
    tokio::task::spawn_blocking(move || {
        if record.archive_bytes.subtype != BinarySubtype::Generic {
            return Err(AppError::NotReady);
        }
        let parsed = package::parse(&record.archive_bytes.bytes).map_err(|_| AppError::NotReady)?;
        if serde_json::to_value(parsed.info()).map_err(|_| AppError::NotReady)?
            != serde_json::to_value(&record.meta.package).map_err(|_| AppError::NotReady)?
        {
            return Err(AppError::NotReady);
        }
        Ok((record.meta, parsed))
    })
    .await
    .map_err(|_| AppError::Internal)?
}
pub async fn revise(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    input: Revise,
) -> Result<(ExtensionView, bool), AppError> {
    dto::request_id(&input.request_id)?;
    dto::version(input.version)?;
    let fingerprint = fingerprint("revise", Some(id), &input)?;
    if let Some(row) = existing(
        database,
        context,
        &input.request_id,
        Some(&fingerprint),
        true,
    )
    .await?
    {
        return Ok((row.view()?, false));
    }
    let old = get_record(database, context, id, true).await?;
    if old.version != input.version {
        return Err(AppError::Conflict("扩展版本已改变".into()));
    }
    let base = if matches!(input.source, Source::Files { .. }) {
        Some(
            load_package(database, context, id, old.revision_id)
                .await?
                .1,
        )
    } else {
        None
    };
    let package = build(input.source, base).await?;
    if package.name != old.name || package.kind != old.kind {
        return Err(AppError::Validation("修订不能改变扩展名称或类型".into()));
    }
    let revision_id = ObjectId::new();
    let now = DateTime::now();
    let (row, created) = transaction::run(
        database,
        (
            database.clone(),
            context.clone(),
            input.request_id,
            fingerprint,
            package,
        ),
        |session, (database, context, request, fingerprint, package)| {
            async move {
                let actor = owned::fresh(database, session, context, policy(), true).await?;
                if let Some(row) =
                    receipt_tx(database, session, &actor, request, Some(fingerprint)).await?
                {
                    return Ok((row, false));
                }
                let mut row = row_tx(database, session, &actor, id, true).await?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("扩展版本已改变")));
                }
                row.version += 1;
                row.revision += 1;
                row.revision_id = revision_id;
                row.package = package.info();
                row.enabled = false;
                row.updated_at = now;
                let changed = database
                    .collection::<Extension>("extensions")
                    .replace_one(
                        doc! {"_id":id,"version":input.version,"deletedAt":null},
                        &row,
                    )
                    .session(&mut *session)
                    .await?;
                if changed.matched_count != 1 {
                    return Err(fail(Failure::Conflict("扩展版本已改变")));
                }
                save_revision(database, session, &actor, &row, request, package).await?;
                save_receipt(database, session, &actor, &row, request, fingerprint).await?;
                shared::audit(database, &actor, "skills.revise", id, session).await?;
                Ok((row, true))
            }
            .boxed()
        },
    )
    .await?;
    Ok((row.view()?, created))
}
pub async fn toggle(
    state: &crate::state::AppState,
    context: &AuthContext,
    id: ObjectId,
    input: Toggle,
) -> Result<ExtensionView, AppError> {
    let database = state.require_database()?;
    dto::version(input.version)?;
    let verified_revision = if input.enabled {
        let row = get_record(database, context, id, true).await?;
        if row.version != input.version {
            return Err(AppError::Conflict("扩展版本已改变".into()));
        }
        let (_, package) = load_package(database, context, id, row.revision_id).await?;
        if serde_json::to_value(&row.package).map_err(|_| AppError::NotReady)?
            != serde_json::to_value(package.info()).map_err(|_| AppError::NotReady)?
        {
            return Err(AppError::NotReady);
        }
        Some((
            row.revision_id,
            serde_json::to_value(package.info()).map_err(|_| AppError::NotReady)?,
        ))
    } else {
        None
    };
    transaction::run(
        database,
        (
            database.clone(),
            context.clone(),
            state.config.clone(),
            verified_revision,
        ),
        |session, (database, context, config, verified_revision)| {
            async move {
                let actor = owned::fresh(database, session, context, policy(), true).await?;
                let mut row = row_tx(database, session, &actor, id, true).await?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("扩展版本已改变")));
                }
                if input.enabled {
                    let Some((revision, package)) = verified_revision.as_ref() else {
                        return Err(fail(Failure::NotReady));
                    };
                    if *revision != row.revision_id {
                        return Err(fail(Failure::Conflict("扩展修订已改变")));
                    }
                    if serde_json::to_value(&row.package).map_err(|_| fail(Failure::NotReady))?
                        != *package
                    {
                        return Err(fail(Failure::NotReady));
                    }
                    super::programs::validate(config, &row.package)
                        .map_err(super::programs::transaction_error)?;
                }
                if input.enabled && !row.package.tools.is_empty() {
                    super::contributions::validate_activation(database, session, &row).await?;
                }
                row.enabled = input.enabled;
                row.version += 1;
                row.updated_at = DateTime::now();
                database
                    .collection::<Extension>("extensions")
                    .replace_one(
                        doc! {"_id":id,"version":input.version,"deletedAt":null},
                        &row,
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, &actor, "skills.toggle", id, session).await?;
                Ok(row)
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn uninstall(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    dto::version(version)?;
    transaction::run(database,(database.clone(),context.clone()),|session,(database,context)|async move{let actor=owned::fresh(database,session,context,policy(),true).await?;let row=row_tx(database,session,&actor,id,true).await?;if row.version!=version{return Err(fail(Failure::Conflict("扩展版本已改变")))}let now=DateTime::now();database.collection::<Document>("extensions").update_one(doc!{"_id":id,"version":version,"deletedAt":null},doc!{"$set":{"deletedAt":now,"updatedAt":now,"enabled":false},"$inc":{"version":1i64}}).session(&mut *session).await?;shared::audit(database,&actor,"skills.uninstall",id,session).await}.boxed()).await
}
pub(super) fn prefix(value: &str) -> String {
    let mut safe = String::from("^");
    for c in value.chars() {
        if ".+*?^${}()|[]\\".contains(c) {
            safe.push('\\')
        }
        safe.push(c)
    }
    safe
}
pub async fn list(
    database: &Database,
    context: &AuthContext,
    query: dto::LibraryQuery,
) -> Result<Page<ExtensionView>, AppError> {
    let uninstalled = matches!(query.status, dto::LibraryStatus::Uninstalled);
    let input = PageInput {
        page: query.page,
        page_size: query.page_size,
        search: query.search,
        sort: query.sort,
    };
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt", "name"])?;
    let limit = input.page_size;
    let mut filter = doc! {"deletedAt":null};
    if uninstalled {
        filter.insert("deletedAt", doc! {"$ne":bson::Bson::Null});
    }
    if let Some(search) = input
        .search
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        filter.insert("name", doc! {"$regex":prefix(search)});
    }
    let(rows,total)=transaction::run_snapshot(database,(database.clone(),context.clone(),filter,sort),|session,(database,context,filter,sort)|async move{let actor=owned::fresh(database,session,context,policy(),false).await?;let mut filter=filter.clone();if actor.scope==DataScope::Own{filter.insert("ownerId",actor.id);}let mut pipeline=vec![doc!{"$match":filter}];if actor.scope==DataScope::Roles{pipeline.push(doc!{"$lookup":{"from":"users","localField":"ownerId","foreignField":"_id","pipeline":[{"$match":{"deletedAt":null}},{"$project":{"roleIds":1}}],"as":"__owner"}});pipeline.push(doc!{"$match":{"$or":[{"ownerId":actor.id},{"__owner.roleIds":{"$in":&actor.scope_role_ids}}]}});}pipeline.push(doc!{"$sort":sort.clone()});pipeline.push(doc!{"$facet":{"rows":[{"$skip":skip as i64},{"$limit":limit as i64},{"$project":{"__owner":0}}],"count":[{"$count":"total"}]}});let mut cursor=database.collection::<Document>("extensions").aggregate(pipeline).max_time(std::time::Duration::from_secs(3)).session(&mut *session).await?;let row=cursor.stream(session).try_next().await?.ok_or_else(||fail(Failure::NotReady))?;let rows=row.get_array("rows").map_err(|_|fail(Failure::NotReady))?.iter().map(|v|bson::deserialize_from_bson::<Extension>(v.clone()).map_err(|_|fail(Failure::NotReady))).collect::<mongodb::error::Result<Vec<_>>>()?;let counts=row.get_array("count").map_err(|_|fail(Failure::NotReady))?;let total=counts.first().and_then(|v|v.as_document()).and_then(|v|v.get("total")).map(|n|n.as_i64().or_else(||n.as_i32().map(i64::from))).unwrap_or(Some(0)).filter(|v|*v>=0).ok_or_else(||fail(Failure::NotReady))? as u64;Ok((rows,total))}.boxed()).await?;
    Ok(Page::new(
        rows.into_iter()
            .map(Extension::view)
            .collect::<Result<_, _>>()?,
        &input,
        total,
    ))
}
pub async fn history(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    input: PageInput,
) -> Result<Page<RevisionView>, AppError> {
    let skip = input.validate()?;
    if input.search.is_some() || input.sort.is_some() {
        return Err(AppError::Validation(
            "修订历史不支持搜索或自定义排序".into(),
        ));
    }
    let size = input.page_size;
    let (rows, total) = transaction::run_snapshot(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                let actor = owned::fresh(database, session, context, policy(), false).await?;
                row_tx(database, session, &actor, id, false).await?;
                let collection = database.collection::<RevisionMeta>("extension_revisions");
                let filter = doc! {"extensionId":id};
                let total = collection
                    .count_documents(filter.clone())
                    .session(&mut *session)
                    .await?;
                let mut cursor = collection
                    .find(filter)
                    .projection(doc! {"archiveBytes":0})
                    .sort(doc! {"number":-1})
                    .skip(skip)
                    .limit(size as i64)
                    .session(&mut *session)
                    .await?;
                let rows = cursor.stream(session).try_collect::<Vec<_>>().await?;
                Ok((rows, total))
            }
            .boxed()
        },
    )
    .await?;
    Ok(Page::new(
        rows.into_iter()
            .map(RevisionMeta::view)
            .collect::<Result<_, _>>()?,
        &input,
        total,
    ))
}
pub async fn preview(
    database: &Database,
    context: &AuthContext,
    id: ObjectId,
    revision: ObjectId,
    path: &str,
) -> Result<PreviewView, AppError> {
    if !package::valid_path(path) {
        return Err(AppError::Validation("预览路径无效".into()));
    }
    let (_, package) = load_package(database, context, id, revision).await?;
    let file = package
        .files
        .iter()
        .find(|file| file.path == path)
        .ok_or(AppError::NotFound)?;
    let content = file
        .text()
        .map_err(|error| AppError::Validation(error.to_string()))?;
    Ok(PreviewView {
        path: file.path.clone(),
        content: content.into(),
        sha256: file.sha256.clone(),
        size: file.bytes.len() as u64,
    })
}
pub async fn constraints(
    database: &Database,
    context: &AuthContext,
) -> Result<ConstraintView, AppError> {
    transaction::run_snapshot(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                owned::fresh(database, session, context, policy(), false).await?;
                database
                    .collection::<Constraint>("extension_constraints")
                    .find_one(doc! {"_id":"global"})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn update_constraints(
    database: &Database,
    context: &AuthContext,
    input: ConstraintInput,
) -> Result<ConstraintView, AppError> {
    dto::version(input.version)?;
    if input.content.len() > 64 * 1024
        || input
            .content
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(AppError::Validation("全局约束内容无效或过大".into()));
    }
    transaction::run(
        database,
        (database.clone(), context.clone(), input),
        |session, (database, context, input)| {
            async move {
                let resource = Resource {
                    write: "skills.constraints.write",
                    ..policy()
                };
                let actor = owned::fresh(database, session, context, resource, true).await?;
                if actor.scope != DataScope::All {
                    return Err(fail(Failure::Forbidden));
                }
                let row = Constraint {
                    version: input.version + 1,
                    content: input.content.clone(),
                    updated_at: DateTime::now(),
                    actor_id: Some(actor.id),
                };
                let result = database
                    .collection::<Constraint>("extension_constraints")
                    .replace_one(doc! {"_id":"global","version":input.version}, &row)
                    .session(&mut *session)
                    .await?;
                if result.matched_count != 1 {
                    return Err(fail(Failure::Conflict("全局约束版本已改变")));
                }
                database
                    .collection::<Constraint>("extension_constraint_revisions")
                    .insert_one(&row)
                    .session(&mut *session)
                    .await?;
                shared::audit(
                    database,
                    &actor,
                    "skills.constraints.update",
                    actor.id,
                    session,
                )
                .await?;
                Ok(row)
            }
            .boxed()
        },
    )
    .await?
    .view()
}
pub async fn constraint_history(
    database: &Database,
    context: &AuthContext,
    input: PageInput,
) -> Result<Page<ConstraintView>, AppError> {
    let skip = input.validate()?;
    if input.search.is_some() || input.sort.is_some() {
        return Err(AppError::Validation(
            "约束历史不支持搜索或自定义排序".into(),
        ));
    }
    let size = input.page_size;
    let (rows, total) = transaction::run_snapshot(
        database,
        (database.clone(), context.clone()),
        |session, (database, context)| {
            async move {
                owned::fresh(database, session, context, policy(), false).await?;
                let collection =
                    database.collection::<Constraint>("extension_constraint_revisions");
                let total = collection
                    .count_documents(doc! {})
                    .session(&mut *session)
                    .await?;
                let mut cursor = collection
                    .find(doc! {})
                    .sort(doc! {"version":-1})
                    .skip(skip)
                    .limit(size as i64)
                    .session(&mut *session)
                    .await?;
                let rows = cursor.stream(session).try_collect::<Vec<_>>().await?;
                Ok((rows, total))
            }
            .boxed()
        },
    )
    .await?;
    Ok(Page::new(
        rows.into_iter()
            .map(Constraint::view)
            .collect::<Result<_, _>>()?,
        &input,
        total,
    ))
}
