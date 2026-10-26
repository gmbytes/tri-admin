use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};
pub const VERSION: i32 = 1;
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"codegen"}).await?;
    if let Some(old) = &previous
        && old.get_i32("version").ok() != Some(VERSION)
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in ["code_definitions", "code_generations", "codegen_packages"] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(error) if error.code==48)
        {
            return Err(error.into());
        }
    }
    for (name, keys, unique, partial) in [
        (
            "code_definitions",
            doc! {"ownerId":1,"requestId":1},
            true,
            None,
        ),
        (
            "code_definitions",
            doc! {"ownerId":1,"schema.resource":1},
            true,
            Some(doc! {"deletedAt":null}),
        ),
        (
            "code_definitions",
            doc! {"ownerId":1,"deletedAt":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "code_definitions",
            doc! {"deletedAt":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "code_generations",
            doc! {"ownerId":1,"requestId":1},
            true,
            None,
        ),
        (
            "code_generations",
            doc! {"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "code_generations",
            doc! {"definitionId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "code_generations",
            doc! {"createdAt":-1,"_id":-1},
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
        .collection::<Document>("_invariants")
        .update_one(
            doc! {"_id":"codegen"},
            doc! {"$setOnInsert":{"version":0i64}},
        )
        .upsert(true)
        .await?;
    if previous.is_none() {
        let path = "/codegen";
        let hash = Sha256::digest(format!("tri-admin:menu:{path}"));
        let mut bytes = [0; 12];
        bytes.copy_from_slice(&hash[..12]);
        let id = mongodb::bson::oid::ObjectId::from_bytes(bytes);
        if database
            .collection::<Document>("menus")
            .find_one(doc! {"path":path,"deletedAt":null})
            .await?
            .is_none()
        {
            let now = DateTime::now();
            database.collection::<Document>("menus").update_one(doc!{"_id":id},doc!{"$setOnInsert":{"label":"代码生成","path":path,"viewKey":"codegen","permission":"codegen.read","icon":null,"parentId":null,"sort":140i32,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
    }
    metadata
        .update_one(
            doc! {"_id":"codegen"},
            doc! {"$setOnInsert":{"version":VERSION,"createdAt":DateTime::now()}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
