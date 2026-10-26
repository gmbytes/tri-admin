use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    for name in [
        "applications",
        "application_actions",
        "application_users",
        "open_sessions",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command)if command.code==48)
        {
            return Err(error.into());
        }
    }
    for (name, keys, unique, partial) in [
        ("applications", doc! {"code":1}, true, None),
        (
            "applications",
            doc! {"createdBy":1,"requestId":1},
            true,
            None,
        ),
        (
            "application_actions",
            doc! {"applicationId":1,"actorId":1,"requestId":1},
            true,
            None,
        ),
        (
            "application_actions",
            doc! {"applicationId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "application_users",
            doc! {"applicationId":1,"usernameKey":1},
            true,
            Some(doc! {"deletedAt":null}),
        ),
        (
            "open_sessions",
            doc! {"applicationId":1,"principalKind":1,"userId":1,"status":1},
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
    for field in ["createdAt", "code", "name"] {
        let mut keys = doc! {"deletedAt":1};
        keys.insert(field, 1);
        keys.insert("_id", 1);
        database
            .collection::<Document>("applications")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    for field in ["createdAt", "usernameKey", "username", "displayName"] {
        let mut keys = doc! {"applicationId":1,"deletedAt":1};
        keys.insert(field, 1);
        keys.insert("_id", 1);
        database
            .collection::<Document>("application_users")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    database
        .collection::<Document>("open_sessions")
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
        .update_one(
            doc! {"_id":"applications"},
            doc! {"$setOnInsert":{"version":0i64}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
