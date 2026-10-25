use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    for name in [
        "data_exports",
        "data_export_attempts",
        "data_export_manifest",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command)if command.code==48)
        {
            return Err(error.into());
        }
    }
    database
        .collection::<mongodb::bson::Document>("_invariants")
        .update_one(
            doc! {"_id":"data-exports"},
            doc! {"$setOnInsert":{"version":1i64,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    database
        .collection::<mongodb::bson::Document>("data_exports")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"actorId":1,"requestId":1})
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    for keys in [
        doc! {"actorId":1,"createdAt":-1,"_id":-1},
        doc! {"actorId":1,"resource":1,"createdAt":-1,"_id":-1},
        doc! {"actorId":1,"state":1,"createdAt":-1,"_id":-1},
        doc! {"state":1,"nextRunAt":1,"leaseExpiresAt":1,"_id":1},
        doc! {"state":1,"expiresAt":1,"_id":1},
        doc! {"storageProfileId":1,"storageRetained":1},
    ] {
        database
            .collection::<mongodb::bson::Document>("data_exports")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    database
        .collection::<mongodb::bson::Document>("data_export_attempts")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"jobId":1,"generation":1})
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    for keys in [
        doc! {"nextRunAt":1,"cleanupExpiresAt":1,"_id":1},
        doc! {"storageProfileId":1,"state":1},
        doc! {"state":1,"nextRunAt":1,"_id":1},
    ] {
        database
            .collection::<mongodb::bson::Document>("data_export_attempts")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    database
        .collection::<mongodb::bson::Document>("data_export_manifest")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"jobId":1,"generation":1,"batch":1})
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    database
        .collection::<mongodb::bson::Document>("data_export_manifest")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"jobId":1,"generation":1,"rowIds":1})
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;
    Ok(())
}
