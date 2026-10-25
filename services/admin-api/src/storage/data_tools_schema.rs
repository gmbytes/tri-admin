use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};
pub const VERSION: i32 = 1;
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"data-tools"}).await?;
    if let Some(record) = &previous
        && record.get_i32("version").ok() != Some(VERSION)
    {
        return Err(AppError::SchemaMismatch);
    }
    if let Err(error) = database.create_collection("data_imports").await
        && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
    {
        return Err(error.into());
    }
    database
        .collection::<Document>("data_imports")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"actorId":1,"requestId":1})
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    for keys in [
        doc! {"actorId":1,"createdAt":-1,"_id":-1},
        doc! {"actorId":1,"expiresAt":1},
    ] {
        database
            .collection::<Document>("data_imports")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    database
        .collection::<Document>("_invariants")
        .update_one(
            doc! {"_id":"data-tools"},
            doc! {"$setOnInsert":{"version":1i64}},
        )
        .upsert(true)
        .await?;
    if previous.is_none() {
        let path = "/data-tools";
        let digest = Sha256::digest(format!("tri-admin:menu:{path}"));
        let mut bytes = [0u8; 12];
        bytes.copy_from_slice(&digest[..12]);
        if database
            .collection::<Document>("menus")
            .find_one(doc! {"path":path,"deletedAt":null})
            .await?
            .is_none()
        {
            let now = DateTime::now();
            database.collection::<Document>("menus").update_one(doc!{"_id":ObjectId::from_bytes(bytes)},doc!{"$setOnInsert":{"label":"数据工具","path":path,"viewKey":"exports","permission":"exports.read","icon":null,"parentId":null,"sort":110,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
    }
    metadata
        .update_one(
            doc! {"_id":"data-tools"},
            doc! {"$setOnInsert":{"version":VERSION,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
