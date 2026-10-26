use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub const VERSION: i32 = 2;
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let old = metadata.find_one(doc! {"_id":"extensions"}).await?;
    if old
        .as_ref()
        .is_some_and(|row| !matches!(row.get_i32("version").ok(), Some(1 | 2)))
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in [
        "extensions",
        "extension_revisions",
        "extension_requests",
        "extension_constraints",
        "extension_constraint_revisions",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(error)if error.code==48)
        {
            return Err(error.into());
        }
    }
    for (name, keys, unique, partial) in [
        (
            "extensions",
            doc! {"ownerId":1,"kind":1,"name":1},
            true,
            Some(doc! {"deletedAt":null}),
        ),
        (
            "extensions",
            doc! {"deletedAt":1,"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "extension_revisions",
            doc! {"extensionId":1,"number":1},
            true,
            None,
        ),
        (
            "extension_requests",
            doc! {"actorId":1,"requestId":1},
            true,
            None,
        ),
        (
            "extension_constraint_revisions",
            doc! {"version":1},
            true,
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
        .collection::<Document>("_invariants")
        .update_one(
            doc! {"_id":"extensions"},
            doc! {"$setOnInsert":{"version":0i64}},
        )
        .upsert(true)
        .await?;
    let initial = doc! {"version":1i64,"content":"","updatedAt":DateTime::now(),"actorId":null};
    database
        .collection::<Document>("extension_constraints")
        .update_one(doc! {"_id":"global"}, doc! {"$setOnInsert":initial.clone()})
        .upsert(true)
        .await?;
    database
        .collection::<Document>("extension_constraint_revisions")
        .update_one(doc! {"version":1i64}, doc! {"$setOnInsert":initial})
        .upsert(true)
        .await?;
    if old.as_ref().and_then(|row| row.get_i32("version").ok()) != Some(VERSION) {
        let path = "/skills";
        if database
            .collection::<Document>("menus")
            .find_one(doc! {"path":path})
            .await?
            .is_none()
        {
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(format!("tri-admin:menu:{path}"));
            let mut bytes = [0; 12];
            bytes.copy_from_slice(&hash[..12]);
            let id = mongodb::bson::oid::ObjectId::from_bytes(bytes);
            let now = DateTime::now();
            database.collection::<Document>("menus").update_one(doc!{"_id":id},doc!{"$setOnInsert":{"label":"Skills 与插件","path":path,"viewKey":"skills","permission":"skills.read","icon":null,"parentId":null,"sort":150i32,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
    }
    metadata
        .update_one(
            doc! {"_id":"extensions"},
            doc! {"$set":{"version":VERSION},"$setOnInsert":{"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
