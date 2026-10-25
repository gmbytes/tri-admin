use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub const VERSION: i32 = 1;
pub async fn migrate_files(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    if let Some(row) = metadata.find_one(doc! {"_id":"files"}).await?
        && row.get_i32("version").ok() != Some(VERSION)
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in [
        "file_categories",
        "files",
        "file_uploads",
        "file_chunks",
        "file_attempts",
        "file_cleanup_tasks",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command)if command.code==48)
        {
            return Err(error.into());
        }
    }
    for (collection, keys, name, partial) in [
        (
            "file_categories",
            doc! {"code":1},
            "active_file_category_code",
            true,
        ),
        (
            "file_uploads",
            doc! {"ownerId":1,"requestId":1},
            "upload_request_unique",
            false,
        ),
        (
            "file_chunks",
            doc! {"uploadId":1,"index":1},
            "upload_chunk_unique",
            false,
        ),
    ] {
        let mut options = IndexOptions::builder()
            .unique(true)
            .name(name.to_owned())
            .build();
        if partial {
            options.partial_filter_expression = Some(doc! {"deletedAt":null})
        }
        database
            .collection::<Document>(collection)
            .create_index(IndexModel::builder().keys(keys).options(options).build())
            .await?;
    }
    for field in ["createdAt", "code", "name"] {
        let mut keys = doc! {"deletedAt":1};
        keys.insert(field, 1);
        keys.insert("_id", 1);
        database
            .collection::<Document>("file_categories")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    for field in ["createdAt", "name"] {
        let mut keys = doc! {"ownerId":1};
        keys.insert(field, 1);
        keys.insert("_id", 1);
        database
            .collection::<Document>("file_uploads")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    for (collection, keys) in [
        ("file_uploads", doc! {"ownerId":1,"state":1}),
        ("file_uploads", doc! {"categoryId":1,"state":1}),
        (
            "file_uploads",
            doc! {"storageProfileId":1,"cleanupPending":1},
        ),
        (
            "file_uploads",
            doc! {"storageProfileId":1,"state":1,"cleanupPending":1},
        ),
        ("files", doc! {"deletedAt":1,"categoryId":1}),
        ("files", doc! {"deletedAt":1,"storageProfileId":1}),
        (
            "file_attempts",
            doc! {"uploadId":1,"state":1,"createdAt":1,"_id":1},
        ),
        ("file_attempts", doc! {"storageProfileId":1,"state":1}),
        ("file_cleanup_tasks", doc! {"storageProfileId":1,"state":1}),
        ("file_cleanup_tasks", doc! {"state":1,"updatedAt":1,"_id":1}),
    ] {
        database
            .collection::<Document>(collection)
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    database
        .collection::<Document>("_invariants")
        .update_one(doc! {"_id":"files"}, doc! {"$setOnInsert":{"version":1i64}})
        .upsert(true)
        .await?;
    metadata
        .update_one(
            doc! {"_id":"files"},
            doc! {"$setOnInsert":{"version":VERSION,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
