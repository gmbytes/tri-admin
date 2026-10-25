use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub async fn migrate_imports(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    if let Some(record) = metadata.find_one(doc! {"_id":"version-imports"}).await?
        && record.get_i32("version").ok() != Some(1)
    {
        return Err(AppError::SchemaMismatch);
    }
    if let Err(error) = database.create_collection("version_previews").await
        && !matches!(&*error.kind,ErrorKind::Command(command)if command.code==48)
    {
        return Err(error.into());
    }
    database
        .collection::<Document>("version_previews")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"cleanupAt":1})
                .options(
                    IndexOptions::builder()
                        .expire_after(std::time::Duration::ZERO)
                        .name("pending_preview_cleanup".to_owned())
                        .build(),
                )
                .build(),
        )
        .await?;
    database
        .collection::<Document>("version_previews")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"actorId":1,"createdAt":-1})
                .build(),
        )
        .await?;
    metadata
        .update_one(
            doc! {"_id":"version-imports"},
            doc! {"$setOnInsert":{"version":1,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
