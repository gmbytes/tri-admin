use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};

pub async fn migrate_dictionaries(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"dictionaries"}).await?;
    if let Some(record) = &previous
        && record.get_i32("version").ok() != Some(1)
    {
        return Err(AppError::SchemaMismatch);
    }
    for collection in ["dictionaries", "dictionary_items"] {
        if let Err(error) = database.create_collection(collection).await
            && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
        {
            return Err(error.into());
        }
    }
    for (collection, keys, name) in [
        (
            "dictionaries",
            doc! {"key":1},
            "active_dictionary_key_unique",
        ),
        (
            "dictionary_items",
            doc! {"dictionaryId":1,"value":1},
            "active_dictionary_value_unique",
        ),
    ] {
        database
            .collection::<Document>(collection)
            .create_index(
                IndexModel::builder()
                    .keys(keys)
                    .options(
                        IndexOptions::builder()
                            .name(name.to_owned())
                            .unique(true)
                            .partial_filter_expression(doc! {"deletedAt":null})
                            .build(),
                    )
                    .build(),
            )
            .await?;
    }
    for (collection, keys) in [
        ("dictionaries", doc! {"deletedAt":1,"createdAt":-1,"_id":-1}),
        ("dictionaries", doc! {"deletedAt":1,"key":1,"_id":1}),
        ("dictionaries", doc! {"deletedAt":1,"name":1,"_id":1}),
        (
            "dictionary_items",
            doc! {"dictionaryId":1,"deletedAt":1,"sort":1,"_id":1},
        ),
        (
            "dictionary_items",
            doc! {"dictionaryId":1,"deletedAt":1,"createdAt":-1,"_id":-1},
        ),
        (
            "dictionary_items",
            doc! {"dictionaryId":1,"deletedAt":1,"value":1,"_id":1},
        ),
        (
            "dictionary_items",
            doc! {"dictionaryId":1,"deletedAt":1,"label":1,"_id":1},
        ),
    ] {
        database
            .collection::<Document>(collection)
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    if previous.is_none() {
        let path = "/dictionaries";
        let digest = Sha256::digest(format!("tri-admin:menu:{path}"));
        let mut bytes = [0u8; 12];
        bytes.copy_from_slice(&digest[..12]);
        let now = DateTime::now();
        if database
            .collection::<Document>("menus")
            .find_one(doc! {"path":path,"deletedAt":null})
            .await?
            .is_none()
        {
            database.collection::<Document>("menus").update_one(doc!{"_id":ObjectId::from_bytes(bytes)},doc!{"$setOnInsert":{"label":"字典管理","path":path,"viewKey":"dictionaries","permission":"dictionaries.read","icon":null,"parentId":null,"sort":50,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
        metadata
            .update_one(
                doc! {"_id":"dictionaries"},
                doc! {"$setOnInsert":{"version":1,"createdAt":now}},
            )
            .upsert(true)
            .await?;
    }
    Ok(())
}
