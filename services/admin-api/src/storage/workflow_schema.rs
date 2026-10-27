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
        .find_one(doc! {"_id":"workflows"})
        .await?
        && row.get_i32("version").ok() != Some(VERSION)
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in [
        "workflow_plans",
        "workflow_plan_revisions",
        "workflow_runs",
        "workflow_payloads",
        "workflow_exports",
        "workflow_export_payloads",
        "workflow_sessions",
        "workflow_session_payloads",
        "workflow_messages",
    ] {
        if let Err(error) = db.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(error)if error.code==48)
        {
            return Err(error.into());
        }
    }
    for (collection, keys, unique, partial) in [
        (
            "workflow_plans",
            doc! {"ownerId":1,"requestId":1},
            true,
            None,
        ),
        (
            "workflow_plans",
            doc! {"ownerId":1,"name":1},
            true,
            Some(doc! {"deletedAt":null}),
        ),
        (
            "workflow_plans",
            doc! {"deletedAt":1,"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "workflow_plans",
            doc! {"deletedAt":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "workflow_plan_revisions",
            doc! {"planId":1,"version":1},
            true,
            None,
        ),
    ] {
        db.collection::<Document>(collection)
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
    db.collection::<Document>("workflow_plan_revisions")
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
    for (keys, unique) in [
        (doc! {"ownerId":1,"requestId":1}, true),
        (doc! {"ownerId":1,"createdAt":-1,"_id":-1}, false),
        (doc! {"createdAt":-1,"_id":-1}, false),
        (doc! {"state":1,"nextRunAt":1,"leaseExpiresAt":1}, false),
        (doc! {"planId":1,"state":1}, false),
        (doc! {"leaseExpiresAt":1}, false),
    ] {
        db.collection::<Document>("workflow_runs")
            .create_index(
                IndexModel::builder()
                    .keys(keys)
                    .options(IndexOptions::builder().unique(unique).build())
                    .build(),
            )
            .await?;
    }
    db.collection::<Document>("workflow_payloads")
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
    db.collection::<Document>("workflow_exports")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"ownerId":1,"requestId":1})
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    db.collection::<Document>("workflow_export_payloads")
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
    for (collection, keys, unique, partial) in [
        (
            "workflow_sessions",
            doc! {"ownerId":1,"requestId":1},
            true,
            None,
        ),
        (
            "workflow_sessions",
            doc! {"deletedAt":1,"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "workflow_sessions",
            doc! {"deletedAt":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "workflow_messages",
            doc! {"ownerId":1,"requestId":1},
            true,
            None,
        ),
        (
            "workflow_messages",
            doc! {"sessionId":1,"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "workflow_messages",
            doc! {"sessionId":1,"parentMessageId":1},
            false,
            None,
        ),
    ] {
        db.collection::<Document>(collection)
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
    db.collection::<Document>("workflow_session_payloads")
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
    db.collection::<Document>("_invariants")
        .update_one(
            doc! {"_id":"workflows"},
            doc! {"$setOnInsert":{"version":0i64}},
        )
        .upsert(true)
        .await?;
    db.collection::<Document>("_schema_migrations")
        .update_one(
            doc! {"_id":"workflows"},
            doc! {"$setOnInsert":{"version":VERSION,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
