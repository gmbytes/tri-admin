use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub async fn migrate_versions(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    if let Some(record) = metadata.find_one(doc! {"_id":"versions"}).await?
        && record.get_i32("version").ok() != Some(1)
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
    metadata
        .update_one(
            doc! {"_id":"versions"},
            doc! {"$setOnInsert":{"version":1,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
