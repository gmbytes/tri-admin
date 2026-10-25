use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};

pub async fn migrate_parameters(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"parameters"}).await?;
    if let Some(record) = &previous
        && record.get_i32("version").ok() != Some(1)
    {
        return Err(AppError::SchemaMismatch);
    }
    if let Err(error) = database.create_collection("parameters").await
        && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
    {
        return Err(error.into());
    }
    database
        .collection::<Document>("parameters")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"key":1})
                .options(
                    IndexOptions::builder()
                        .name("active_parameter_key_unique".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! {"deletedAt":null})
                        .build(),
                )
                .build(),
        )
        .await?;
    for field in ["createdAt", "key", "name", "group"] {
        let mut keys = doc! {"deletedAt":1};
        keys.insert(field, 1);
        keys.insert("_id", 1);
        database
            .collection::<Document>("parameters")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    if previous.is_none() {
        let path = "/parameters";
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
            database.collection::<Document>("menus").update_one(doc!{"_id":ObjectId::from_bytes(bytes)},doc!{"$setOnInsert":{"label":"参数管理","path":path,"viewKey":"parameters","permission":"parameters.read","icon":null,"parentId":null,"sort":60,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
        metadata
            .update_one(
                doc! {"_id":"parameters"},
                doc! {"$setOnInsert":{"version":1,"createdAt":now}},
            )
            .upsert(true)
            .await?;
    }
    Ok(())
}
