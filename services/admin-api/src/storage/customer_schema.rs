use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};

pub async fn migrate_customers(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"customers"}).await?;
    if let Some(record) = &previous
        && record.get_i32("version").ok() != Some(1)
    {
        return Err(AppError::SchemaMismatch);
    }
    if let Err(error) = database.create_collection("customers").await
        && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
    {
        return Err(error.into());
    }
    database
        .collection::<Document>("customers")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"codeKey":1})
                .options(
                    IndexOptions::builder()
                        .name("active_customer_code_unique".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! {"deletedAt":null})
                        .build(),
                )
                .build(),
        )
        .await?;
    for keys in [
        doc! {"deletedAt":1,"createdAt":-1,"_id":-1},
        doc! {"ownerId":1,"deletedAt":1,"createdAt":-1,"_id":-1},
        doc! {"deletedAt":1,"status":1,"createdAt":-1,"_id":-1},
        doc! {"ownerId":1,"deletedAt":1,"status":1,"createdAt":-1,"_id":-1},
    ] {
        database
            .collection::<Document>("customers")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    for field in ["nameKey", "codeKey", "status", "updatedAt"] {
        let mut all = doc! {"deletedAt":1};
        all.insert(field, 1);
        all.insert("_id", 1);
        let mut own = doc! {"ownerId":1,"deletedAt":1};
        own.insert(field, 1);
        own.insert("_id", 1);
        for keys in [all, own] {
            database
                .collection::<Document>("customers")
                .create_index(IndexModel::builder().keys(keys).build())
                .await?;
        }
    }
    if previous.is_none() {
        let path = "/customers";
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
            database.collection::<Document>("menus").update_one(doc!{"_id":ObjectId::from_bytes(bytes)},doc!{"$setOnInsert":{"label":"客户管理","path":path,"viewKey":"customers","permission":"customers.read","icon":null,"parentId":null,"sort":70,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
        metadata
            .update_one(
                doc! {"_id":"customers"},
                doc! {"$setOnInsert":{"version":1,"createdAt":now}},
            )
            .upsert(true)
            .await?;
    }
    Ok(())
}
