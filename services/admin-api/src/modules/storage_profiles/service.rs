use super::{dto::*, models::*, validate};
use crate::common::pagination::{Page, PageInput};
use crate::{
    common::{
        canonical,
        error::AppError,
        transaction::{Failure, fail},
        vault,
    },
    config::Config,
};
use futures_util::TryStreamExt;
use mongodb::{
    ClientSession, Database,
    bson::{doc, oid::ObjectId},
};
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOption {
    pub id: String,
    pub name: String,
    pub code: String,
    pub version: i64,
}
impl Profile {
    fn option(self) -> StorageOption {
        StorageOption {
            id: self.id.to_hex(),
            name: self.name,
            code: self.code,
            version: self.version,
        }
    }
}
pub async fn options(db: &Database, input: PageInput) -> Result<Page<StorageOption>, AppError> {
    let skip = input.validate()?;
    let mut filter = doc! {"deletedAt":null,"enabled":true,"kind":{"$in":["localStorage","s3"]}};
    if let Some(search) = &input.search {
        if search.chars().count() > 100 {
            return Err(AppError::Validation("搜索内容过长".into()));
        }
        let mut prefix = String::from("^");
        for c in search.chars() {
            if ".+*?^${}()|[]\\".contains(c) {
                prefix.push('\\')
            }
            prefix.push(c)
        }
        filter.insert(
            "$or",
            vec![
                doc! {"name":{"$regex":prefix.clone()}},
                doc! {"code":{"$regex":prefix}},
            ],
        );
    }
    let collection = db.collection::<Profile>("integration_profiles");
    let total = collection.count_documents(filter.clone()).await?;
    let rows = collection
        .find(filter)
        .sort(doc! {"code":1,"_id":1})
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    Ok(Page::new(
        rows.into_iter().map(Profile::option).collect(),
        &input,
        total,
    ))
}
pub async fn option(db: &Database, id: ObjectId) -> Result<StorageOption, AppError> {
    Ok(db
        .collection::<Profile>("integration_profiles")
        .find_one(
            doc! {"_id":id,"deletedAt":null,"enabled":true,"kind":{"$in":["localStorage","s3"]}},
        )
        .await?
        .ok_or(AppError::NotFound)?
        .option())
}
fn tx(error: AppError) -> mongodb::error::Error {
    match error {
        AppError::SecretStoreUnavailable => fail(Failure::SecretStoreUnavailable),
        AppError::Validation(_) => fail(Failure::Validation("文件存储配置或凭据无效")),
        _ => fail(Failure::NotReady),
    }
}
fn aad(id: ObjectId, kind: Kind, configuration_json: &str) -> String {
    format!(
        "tri-admin/settings/profile/{}/{}/{}",
        id.to_hex(),
        kind.as_str(),
        canonical::checksum(configuration_json.as_bytes())
    )
}
pub(super) fn decrypt(config: &Config, row: &Profile) -> Result<Option<Credentials>, AppError> {
    let Some(sealed) = &row.secret_box else {
        return Ok(None);
    };
    let bytes = vault::open(
        config,
        aad(row.id, row.kind, &row.configuration_json).as_bytes(),
        sealed,
    )?;
    let value: Credentials =
        serde_json::from_slice(&bytes).map_err(|_| AppError::SecretStoreUnavailable)?;
    if value.kind() != row.kind {
        return Err(AppError::SecretStoreUnavailable);
    }
    Ok(Some(value))
}
pub(crate) async fn storage_definition(
    database: &Database,
    config: &Config,
    id: ObjectId,
) -> Result<(Integration, Option<Credentials>), AppError> {
    let row = database
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":id,"deletedAt":null,"enabled":true})
        .await?
        .ok_or(AppError::NotFound)?;
    if !row.kind.storage() {
        return Err(AppError::Validation("所选配置不是文件存储".into()));
    }
    let public = row.configuration()?;
    let secret = decrypt(config, &row)?;
    validate::credentials(&public, secret.as_ref(), true)?;
    Ok((public, secret))
}
pub(crate) async fn binding(
    database: &Database,
    id: Option<ObjectId>,
    storage: bool,
    config: &Config,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let Some(id) = id else {
        return Ok(());
    };
    let row = database
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":id,"deletedAt":null,"enabled":true})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Validation("默认绑定配置不存在或未启用")))?;
    if !storage || !row.kind.storage() {
        return Err(fail(Failure::Validation("默认绑定的配置类型不匹配")));
    }
    let public = row.configuration().map_err(tx)?;
    let secret = decrypt(config, &row).map_err(tx)?;
    validate::credentials(&public, secret.as_ref(), true).map_err(tx)
}
