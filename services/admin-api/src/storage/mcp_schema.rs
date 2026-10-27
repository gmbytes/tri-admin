use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub const VERSION: i32 = 1;
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    if let Some(old) = metadata.find_one(doc! {"_id":"mcp"}).await?
        && old.get_i32("version").ok() != Some(VERSION)
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in [
        "mcp_connections",
        "mcp_instances",
        "mcp_actions",
        "mcp_results",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(error)if error.code==48)
        {
            return Err(error.into());
        }
    }
    for (name, keys, unique, partial) in [
        (
            "mcp_connections",
            doc! {"ownerId":1,"requestId":1},
            true,
            None,
        ),
        (
            "mcp_connections",
            doc! {"ownerId":1,"name":1},
            true,
            Some(doc! {"deletedAt":null}),
        ),
        (
            "mcp_connections",
            doc! {"deletedAt":1,"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "mcp_instances",
            doc! {"connectionId":1,"createdAt":-1},
            false,
            None,
        ),
        (
            "mcp_instances",
            doc! {"exitConfirmed":1,"leaseUntil":1},
            false,
            None,
        ),
        ("mcp_actions", doc! {"actorId":1,"requestId":1}, true, None),
        (
            "mcp_actions",
            doc! {"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "mcp_actions",
            doc! {"connectionId":1,"status":1,"createdAt":1},
            false,
            None,
        ),
    ] {
        database
            .collection::<Document>(name)
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
    database
        .collection::<Document>("mcp_results")
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
    database
        .collection::<Document>("_invariants")
        .update_one(doc! {"_id":"mcp"}, doc! {"$setOnInsert":{"version":0i64}})
        .upsert(true)
        .await?;
    metadata
        .update_one(
            doc! {"_id":"mcp"},
            doc! {"$setOnInsert":{"version":VERSION,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
