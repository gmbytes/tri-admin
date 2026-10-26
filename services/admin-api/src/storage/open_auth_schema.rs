use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    for name in [
        "open_refresh_tokens",
        "application_usage_buckets",
        "application_call_logs",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command)if command.code==48)
        {
            return Err(error.into());
        }
    }
    for (name, keys) in [
        (
            "open_refresh_tokens",
            doc! {"applicationId":1,"sessionId":1},
        ),
        (
            "application_call_logs",
            doc! {"applicationId":1,"createdAt":-1,"_id":-1},
        ),
        (
            "application_call_logs",
            doc! {"applicationId":1,"userId":1,"createdAt":-1,"_id":-1},
        ),
        ("application_call_logs", doc! {"createdAt":1,"_id":1}),
        (
            "application_call_logs",
            doc! {"applicationId":1,"principalKind":1,"status":1,"createdAt":-1,"_id":-1},
        ),
        (
            "application_call_logs",
            doc! {"applicationId":1,"state":1,"createdAt":-1,"_id":-1},
        ),
    ] {
        database
            .collection::<Document>(name)
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    for (name, key) in [
        ("open_refresh_tokens", "expiresAt"),
        ("application_usage_buckets", "expireAt"),
        ("application_call_logs", "expireAt"),
    ] {
        database
            .collection::<Document>(name)
            .create_index(
                IndexModel::builder()
                    .keys({
                        let mut keys = Document::new();
                        keys.insert(key, 1);
                        keys
                    })
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
        .collection::<Document>("open_sessions")
        .update_many(
            doc! {"schemaVersion":{"$ne":1},"status":"active"},
            doc! {"$set":{"status":"revoked","revokedAt":DateTime::now()}},
        )
        .await?;
    Ok(())
}
