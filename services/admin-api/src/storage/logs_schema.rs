use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};
pub const VERSION: i32 = 2;
pub async fn migrate_logs(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"logs"}).await?;
    if let Some(record) = &previous
        && !matches!(record.get_i32("version").ok(), Some(1 | VERSION))
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in ["request_errors", "log_cleanups"] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command)if command.code==48)
        {
            return Err(error.into());
        }
    }
    if previous.is_none() {
        database
            .collection::<Document>("audit_events")
            .update_many(
                doc! {"kind":{"$exists":false},"action":"auth.login"},
                doc! {"$set":{"kind":"login"}},
            )
            .await?;
        database
            .collection::<Document>("audit_events")
            .update_many(
                doc! {"kind":{"$exists":false}},
                doc! {"$set":{"kind":"operation"}},
            )
            .await?;
    }
    for (collection, prefix) in [("audit_events", Some("kind")), ("request_errors", None)] {
        for field in [
            None,
            Some("action"),
            Some("actorId"),
            Some("requestId"),
            Some("outcome"),
            Some("status"),
            Some("username"),
        ] {
            if collection == "request_errors" && field == Some("username") {
                continue;
            }
            let mut keys = Document::new();
            if let Some(prefix) = prefix {
                keys.insert(prefix, 1);
            }
            if let Some(field) = field {
                keys.insert(field, 1);
            }
            keys.insert("createdAt", -1);
            keys.insert("_id", -1);
            database
                .collection::<Document>(collection)
                .create_index(IndexModel::builder().keys(keys).build())
                .await?;
        }
    }
    database
        .collection::<Document>("log_cleanups")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"requestId":1})
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .name("cleanup_request_unique".to_owned())
                        .build(),
                )
                .build(),
        )
        .await?;
    database
        .collection::<Document>("_invariants")
        .update_one(doc! {"_id":"logs"}, doc! {"$setOnInsert":{"version":1i64}})
        .upsert(true)
        .await?;
    if previous
        .as_ref()
        .and_then(|record| record.get_i32("version").ok())
        != Some(VERSION)
    {
        let path = "/logs";
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
            database.collection::<Document>("menus").update_one(doc!{"_id":ObjectId::from_bytes(bytes)},doc!{"$setOnInsert":{"label":"日志中心","path":path,"viewKey":"audit","permission":"audit.read","icon":null,"parentId":null,"sort":80,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
        metadata
            .update_one(
                doc! {"_id":"logs"},
                doc! {"$set":{"version":VERSION},"$setOnInsert":{"createdAt":now}},
            )
            .upsert(true)
            .await?;
    }
    Ok(())
}
