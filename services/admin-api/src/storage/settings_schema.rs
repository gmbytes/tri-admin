use crate::{
    common::error::AppError,
    modules::settings::{dto::Integration, models::stable_id},
};
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};
pub const VERSION: i32 = 3;
pub async fn migrate_settings(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"settings"}).await?;
    if let Some(row) = &previous
        && !matches!(row.get_i32("version").ok(), Some(1 | 2 | VERSION))
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in [
        "service_settings",
        "integration_profiles",
        "configuration_checks",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command)if command.code==48)
        {
            return Err(error.into());
        }
    }
    database
        .collection::<Document>("integration_profiles")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"code":1})
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .partial_filter_expression(doc! {"deletedAt":null})
                        .name("active_profile_code_unique".to_owned())
                        .build(),
                )
                .build(),
        )
        .await?;
    for field in ["createdAt", "name", "code"] {
        for kind in [false, true] {
            let mut keys = doc! {"deletedAt":1};
            if kind {
                keys.insert("kind", 1);
            }
            keys.insert(field, 1);
            keys.insert("_id", 1);
            database
                .collection::<Document>("integration_profiles")
                .create_index(IndexModel::builder().keys(keys).build())
                .await?;
        }
    }
    let now = DateTime::now();
    let json = crate::common::canonical::json(&Integration::LocalStorage {
        subdirectory: "managed".into(),
    })?;
    database.collection::<Document>("integration_profiles").update_one(doc!{"_id":stable_id("local-default")},doc!{"$setOnInsert":{"name":"默认本地存储","code":"local-default","description":"服务器存储根下的受控目录","kind":"localStorage","configurationJson":json,"secretBox":null,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
    database.collection::<Document>("service_settings").update_one(doc!{"_id":stable_id("service")},doc!{"$setOnInsert":{"displayName":"TRI ADMIN","loginLimitPerIpOverride":null,"storageProfileId":stable_id("local-default"),"mailProfileId":null,"version":1i64,"createdAt":now,"updatedAt":now}}).upsert(true).await?;
    database
        .collection::<Document>("_invariants")
        .update_one(
            doc! {"_id":"settings"},
            doc! {"$setOnInsert":{"version":1i64}},
        )
        .upsert(true)
        .await?;
    for keys in [
        doc! {"profileId":1,"createdAt":-1,"_id":-1},
        doc! {"actorId":1,"createdAt":-1,"_id":-1},
    ] {
        database
            .collection::<Document>("configuration_checks")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    if previous
        .as_ref()
        .and_then(|row| row.get_i32("version").ok())
        != Some(VERSION)
    {
        let path = "/settings";
        let digest = Sha256::digest(format!("tri-admin:menu:{path}"));
        let mut bytes = [0u8; 12];
        bytes.copy_from_slice(&digest[..12]);
        if database
            .collection::<Document>("menus")
            .find_one(doc! {"path":path,"deletedAt":null})
            .await?
            .is_none()
        {
            database.collection::<Document>("menus").update_one(doc! {"_id":ObjectId::from_bytes(bytes)},doc! {"$setOnInsert":{"label":"配置中心","path":path,"viewKey":"settings","permission":"settings.read","icon":null,"parentId":null,"sort":90,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
    }
    metadata
        .update_one(
            doc! {"_id":"settings"},
            doc! {"$set":{"version":VERSION},"$setOnInsert":{"createdAt":now}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
