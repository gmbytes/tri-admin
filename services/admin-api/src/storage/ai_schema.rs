use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub const VERSION: i32 = 1;
pub async fn migrate(db: &Database) -> Result<(), AppError> {
    if let Some(row) = db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"ai"})
        .await?
        && row.get_i32("version").ok() != Some(VERSION)
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in ["ai_providers", "ai_runs", "ai_inputs", "ai_states"] {
        if let Err(error) = db.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(error) if error.code==48)
        {
            return Err(error.into());
        }
    }
    db.collection::<Document>("ai_providers")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"name":1})
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .partial_filter_expression(doc! {"deletedAt":null})
                        .build(),
                )
                .build(),
        )
        .await?;
    for keys in [
        doc! {"deletedAt":1,"createdAt":-1,"_id":-1},
        doc! {"deletedAt":1,"name":1,"_id":1},
        doc! {"profileId":1,"deletedAt":1},
    ] {
        db.collection::<Document>("ai_providers")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    for (keys, unique, partial) in [
        (doc! {"ownerId":1,"requestId":1}, true, None),
        (
            doc! {"ownerId":1,"cancelRequestId":1},
            true,
            Some(doc! {"cancelRequestId":{"$type":"string"}}),
        ),
        (
            doc! {"ownerId":1,"state":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (doc! {"createdAt":-1,"_id":-1}, false, None),
        (
            doc! {"state":1,"nextRunAt":1,"leaseExpiresAt":1,"createdAt":1},
            false,
            None,
        ),
        (doc! {"providerId":1,"state":1}, false, None),
        (doc! {"leaseExpiresAt":1}, false, None),
        (
            doc! {"state":1,"executionDeadline":1,"leaseExpiresAt":1},
            false,
            None,
        ),
        (doc! {"expiresAt":1}, false, None),
    ] {
        db.collection::<Document>("ai_runs")
            .create_index(
                IndexModel::builder()
                    .keys(keys)
                    .options(
                        IndexOptions::builder()
                            .unique(unique)
                            .partial_filter_expression(partial)
                            .build(),
                    )
                    .build(),
            )
            .await?;
    }
    for name in ["ai_inputs", "ai_states"] {
        db.collection::<Document>(name)
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"expiresAt":1})
                    .options(
                        IndexOptions::builder()
                            .expire_after(std::time::Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await?;
    }
    db.collection::<Document>("_invariants")
        .update_one(doc! {"_id":"ai"}, doc! {"$setOnInsert":{"version":0i64}})
        .upsert(true)
        .await?;
    db.collection::<Document>("_schema_migrations")
        .update_one(
            doc! {"_id":"ai"},
            doc! {"$setOnInsert":{"version":VERSION,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
