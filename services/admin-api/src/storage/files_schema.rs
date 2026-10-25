use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub const VERSION: i32 = 3;
pub async fn migrate_files(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    if let Some(row) = metadata.find_one(doc! {"_id":"files"}).await?
        && !matches!(row.get_i32("version").ok(), Some(1 | 2 | VERSION))
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
        "file_storage_probes",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command)if command.code==48)
        {
            return Err(error.into());
        }
    }
    for (collection, keys, name, partial) in [
        ("files", doc! {"blobId":1}, "published_blob_unique", false),
        (
            "file_attempts",
            doc! {"blobId":1},
            "candidate_blob_unique",
            false,
        ),
        (
            "files",
            doc! {"uploadId":1},
            "published_upload_unique",
            false,
        ),
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
        ("file_storage_probes", doc! {"storageProfileId":1,"state":1}),
        (
            "file_storage_probes",
            doc! {"state":1,"nextRunAt":1,"createdAt":1,"_id":1},
        ),
        ("file_storage_probes", doc! {"state":1,"leaseUntil":1}),
        (
            "file_attempts",
            doc! {"kind":1,"state":1,"recoveryUntil":1,"nextRunAt":1,"createdAt":1,"_id":1},
        ),
        ("file_chunks", doc! {"blobId":1,"state":1}),
        ("file_uploads", doc! {"state":1,"createdAt":1,"_id":1}),
        (
            "file_attempts",
            doc! {"kind":1,"state":1,"leaseUntil":1,"createdAt":1,"_id":1},
        ),
        (
            "file_attempts",
            doc! {"uploadId":1,"kind":1,"state":1,"createdAt":1,"_id":1},
        ),
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
        (
            "file_cleanup_tasks",
            doc! {"state":1,"nextRunAt":1,"updatedAt":1,"_id":1},
        ),
    ] {
        database
            .collection::<Document>(collection)
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    for field in ["createdAt", "name"] {
        for own in [false, true] {
            for category in [false, true] {
                let mut keys = doc! {"deletedAt":1};
                if own {
                    keys.insert("ownerId", 1);
                }
                if category {
                    keys.insert("categoryId", 1);
                }
                keys.insert(field, 1);
                keys.insert("_id", 1);
                database
                    .collection::<Document>("files")
                    .create_index(IndexModel::builder().keys(keys).build())
                    .await?;
            }
        }
    }
    database
        .collection::<Document>("file_chunks")
        .update_many(
            doc! {"state":{"$exists":false}},
            doc! {"$set":{"state":"accepted"}},
        )
        .await?;
    database.collection::<Document>("file_uploads").update_many(doc! {"publishAttemptId":{"$exists":false}},doc! {"$set":{"publishAttemptId":null,"fileId":null,"lastError":null,"completedAt":null}}).await?;
    database
        .collection::<Document>("file_cleanup_tasks")
        .update_many(
            doc! {"nextRunAt":{"$exists":false}},
            doc! {"$set":{"nextRunAt":DateTime::now()}},
        )
        .await?;
    database
        .collection::<Document>("_invariants")
        .update_one(doc! {"_id":"files"}, doc! {"$setOnInsert":{"version":1i64}})
        .upsert(true)
        .await?;
    database
        .collection::<Document>("file_attempts")
        .update_many(
            doc! {"kind":"chunk","nextRunAt":{"$exists":false}},
            doc! {"$set":{"nextRunAt":DateTime::now()}},
        )
        .await?;
    database
        .collection::<Document>("file_attempts")
        .update_many(
            doc! {"kind":"publish","retryRequested":{"$exists":false}},
            doc! {"$set":{"retryRequested":false}},
        )
        .await?;
    metadata
        .update_one(
            doc! {"_id":"files"},
            doc! {"$set":{"version":VERSION},"$setOnInsert":{"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
