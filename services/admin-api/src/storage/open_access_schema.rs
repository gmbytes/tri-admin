use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub const VERSION: i32 = 3;
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    if let Some(previous) = metadata.find_one(doc! {"_id":"open-access"}).await?
        && !matches!(
            previous.get_i32("version").ok(),
            Some(1) | Some(2) | Some(VERSION)
        )
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in [
        "api_tokens",
        "api_usage_buckets",
        "api_call_logs",
        "open_auth_attempts",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
        {
            return Err(error.into());
        }
    }
    for (name, keys, unique) in [
        ("api_tokens", doc! {"ownerId":1,"requestId":1}, true),
        (
            "api_tokens",
            doc! {"ownerId":1,"createdAt":-1,"_id":-1},
            false,
        ),
        ("api_tokens", doc! {"ownerId":1,"name":1,"_id":1}, false),
        (
            "api_tokens",
            doc! {"ownerId":1,"revokedAt":1,"expiresAt":1},
            false,
        ),
        (
            "api_call_logs",
            doc! {"ownerId":1,"createdAt":-1,"_id":-1},
            false,
        ),
        (
            "api_call_logs",
            doc! {"ownerId":1,"tokenId":1,"createdAt":-1,"_id":-1},
            false,
        ),
        (
            "api_call_logs",
            doc! {"ownerId":1,"status":1,"createdAt":-1,"_id":-1},
            false,
        ),
        ("api_call_logs", doc! {"createdAt":1,"_id":1}, false),
    ] {
        database
            .collection::<Document>(name)
            .create_index(
                IndexModel::builder()
                    .keys(keys)
                    .options(IndexOptions::builder().unique(unique).build())
                    .build(),
            )
            .await?;
    }
    for name in ["api_usage_buckets", "api_call_logs", "open_auth_attempts"] {
        database
            .collection::<Document>(name)
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"expireAt":1})
                    .options(
                        IndexOptions::builder()
                            .expire_after(std::time::Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await?;
    }
    database
        .collection::<Document>("_invariants")
        .update_one(
            doc! {"_id":"api-tokens"},
            doc! {"$setOnInsert":{"version":0i64}},
        )
        .upsert(true)
        .await?;
    super::application_schema::migrate(database).await?;
    super::open_auth_schema::migrate(database).await?;
    metadata
        .update_one(
            doc! {"_id":"open-access"},
            doc! {"$setOnInsert":{"version":VERSION,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    metadata
        .update_one(
            doc! {"_id":"open-access","version":{"$in":[1,2]}},
            doc! {"$set":{"version":VERSION,"updatedAt":DateTime::now()}},
        )
        .await?;
    Ok(())
}
