use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};
pub async fn migrate_versions(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"versions"}).await?;
    if let Some(record) = &previous
        && !matches!(record.get_i32("version").ok(), Some(1 | 2))
    {
        return Err(AppError::SchemaMismatch);
    }
    for collection in ["versions", "version_packages"] {
        if let Err(error) = database.create_collection(collection).await
            && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
        {
            return Err(error.into());
        }
    }
    database
        .collection::<Document>("versions")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"codeKey":1})
                .options(
                    IndexOptions::builder()
                        .name("version_code_history_unique".to_owned())
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await?;
    for field in ["createdAt", "name", "codeKey"] {
        let mut keys = doc! {"deletedAt":1};
        keys.insert(field, 1);
        keys.insert("_id", 1);
        database
            .collection::<Document>("versions")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    database
        .collection::<Document>("_invariants")
        .update_one(
            doc! {"_id":"versions"},
            doc! {"$setOnInsert":{"version":1i64}},
        )
        .upsert(true)
        .await?;
    if previous
        .as_ref()
        .and_then(|record| record.get_i32("version").ok())
        != Some(2)
    {
        let path = "/versions";
        let digest = Sha256::digest(format!("tri-admin:menu:{path}"));
        let mut bytes = [0u8; 12];
        bytes.copy_from_slice(&digest[..12]);
        let now = DateTime::now();
        if database
            .collection::<Document>("menus")
            .find_one(doc! {"path":path,"deletedAt":null})
            .await?
            .is_none()
        {
            database.collection::<Document>("menus").update_one(doc!{"_id":ObjectId::from_bytes(bytes)},doc!{"$setOnInsert":{"label":"版本管理","path":path,"viewKey":"versions","permission":"versions.read","icon":null,"parentId":null,"sort":70,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
        metadata
            .update_one(
                doc! {"_id":"versions"},
                doc! {"$set":{"version":2},"$setOnInsert":{"createdAt":now}},
            )
            .upsert(true)
            .await?;
    }
    Ok(())
}
