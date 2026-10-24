use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};

pub async fn migrate_auth(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    if let Some(record) = metadata.find_one(doc! {"_id":"auth"}).await?
        && record.get_i32("version").ok() != Some(1)
    {
        return Err(AppError::SchemaMismatch);
    }
    for collection in ["refresh_tokens", "login_attempts", "audit_events"] {
        if let Err(error) = database.create_collection(collection).await
            && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
        {
            return Err(error.into());
        }
    }
    for collection in ["refresh_tokens", "login_attempts"] {
        let field = if collection == "login_attempts" {
            "resetAt"
        } else {
            "expiresAt"
        };
        let mut keys = Document::new();
        keys.insert(field, 1);
        database
            .collection::<Document>(collection)
            .create_index(
                IndexModel::builder()
                    .keys(keys)
                    .options(
                        IndexOptions::builder()
                            .name("expiry".to_owned())
                            .expire_after(std::time::Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await?;
    }
    database
        .collection::<Document>("audit_events")
        .create_index(IndexModel::builder().keys(doc! {"createdAt":-1}).build())
        .await?;
    metadata
        .update_one(
            doc! {"_id":"auth"},
            doc! {"$setOnInsert":{"version":1,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
