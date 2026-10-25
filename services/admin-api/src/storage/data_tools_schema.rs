use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};
pub const VERSION: i32 = 6;
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"data-tools"}).await?;
    if let Some(record) = &previous
        && !matches!(
            record.get_i32("version").ok(),
            Some(1 | 2 | 3 | 4 | 5 | VERSION)
        )
    {
        return Err(AppError::SchemaMismatch);
    }
    if previous
        .as_ref()
        .and_then(|row| row.get_i32("version").ok())
        == Some(1)
    {
        database.collection::<Document>("data_imports").update_many(doc!{},vec![doc!{"$set":{"rows":{"$map":{"input":"$rows","as":"row","in":{"$mergeObjects":[{"parentId":null},"$$row"]}}}}}]).await?;
    }
    if let Err(error) = database.create_collection("data_imports").await
        && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
    {
        return Err(error.into());
    }
    if previous
        .as_ref()
        .and_then(|row| row.get_i32("version").ok())
        .is_some_and(|version| version < 3)
    {
        let count = |status: &str| doc! {"$size":{"$filter":{"input":"$rows","as":"row","cond":{"$eq":["$$row.status",status]}}}};
        database.collection::<Document>("data_imports").update_many(doc!{},vec![doc!{"$set":{"rowCount":{"$size":"$rows"},"readyCount":count("ready"),"appliedCount":count("applied"),"expiredCount":count("expired"),"payloadState":"full","redactedAt":null}},doc!{"$set":{"errorCount":{"$subtract":["$rowCount",{"$add":["$readyCount","$appliedCount"]}]}}}]).await?;
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
    if previous
        .as_ref()
        .and_then(|row| row.get_i32("version").ok())
        .is_some_and(|version| version < 4)
    {
        database
            .collection::<Document>("data_imports")
            .update_many(
                doc! {},
                vec![doc! {"$set":{"template":{"$ifNull":["$template",null]}}}],
            )
            .await?;
    }
    if let Err(error) = database.create_collection("export_templates").await
        && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
    {
        return Err(error.into());
    }
    database
        .collection::<Document>("export_templates")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"code":1})
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .partial_filter_expression(doc! {"deletedAt":null})
                        .build(),
                )
                .build(),
        )
        .await?;
    for field in ["createdAt", "name", "code"] {
        for scoped in [false, true] {
            let mut keys = doc! {"deletedAt":1};
            if scoped {
                keys.insert("resource", 1);
            }
            keys.insert(field, 1);
            keys.insert("_id", 1);
            database
                .collection::<Document>("export_templates")
                .create_index(IndexModel::builder().keys(keys).build())
                .await?;
        }
    }
    for keys in [
        doc! {"actorId":1,"createdAt":-1,"_id":-1},
        doc! {"actorId":1,"expiresAt":1},
        doc! {"actorId":1,"resource":1,"createdAt":-1,"_id":-1},
        doc! {"actorId":1,"readyCount":1,"expiresAt":1},
        doc! {"payloadState":1,"expiresAt":1,"_id":1},
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
    super::export_job_schema::migrate(database).await?;
    if previous
        .as_ref()
        .and_then(|row| row.get_i32("version").ok())
        .is_some_and(|version| version < 6)
    {
        database
            .collection::<Document>("data_export_attempts")
            .update_many(
                doc! {},
                vec![doc! {"$set":{"retryRequested":{"$ifNull":["$retryRequested",false]}}}],
            )
            .await?;
    }
    metadata
        .update_one(
            doc! {"_id":"data-tools"},
            doc! {"$set":{"version":VERSION},"$setOnInsert":{"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
