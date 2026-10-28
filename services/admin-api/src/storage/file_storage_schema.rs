use crate::{
    common::{canonical, error::AppError},
    modules::storage_profiles::{dto::Integration, models::stable_id},
};
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub async fn migrate(db: &Database) -> Result<(), AppError> {
    if let Some(row) = db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"file-storage"})
        .await?
        && row.get_i32("version").ok() != Some(1)
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in ["integration_profiles", "service_settings"] {
        if let Err(error) = db.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(error) if error.code==48)
        {
            return Err(error.into());
        }
    }
    db.collection::<Document>("integration_profiles")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"code":1})
                .options(
                    IndexOptions::builder()
                        .name("active_profile_code_unique".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! {"deletedAt":null})
                        .build(),
                )
                .build(),
        )
        .await?;
    db.collection::<Document>("integration_profiles")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"deletedAt":1,"kind":1,"code":1,"_id":1})
                .build(),
        )
        .await?;
    let now = DateTime::now();
    let configuration = canonical::json(&Integration::LocalStorage {
        subdirectory: "managed".into(),
    })?;
    db.collection::<Document>("integration_profiles").update_one(doc!{"_id":stable_id("local-default")},doc!{"$setOnInsert":{"name":"默认本地存储","code":"local-default","kind":"localStorage","configurationJson":configuration,"secretBox":null,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
    db.collection::<Document>("service_settings").update_one(doc!{"_id":stable_id("service")},doc!{"$setOnInsert":{"storageProfileId":stable_id("local-default"),"version":1i64,"createdAt":now,"updatedAt":now}}).upsert(true).await?;
    db.collection::<Document>("_invariants")
        .update_one(
            doc! {"_id":"file-storage"},
            doc! {"$setOnInsert":{"version":1i64}},
        )
        .upsert(true)
        .await?;
    db.collection::<Document>("_schema_migrations")
        .update_one(
            doc! {"_id":"file-storage"},
            doc! {"$setOnInsert":{"version":1,"createdAt":now}},
        )
        .upsert(true)
        .await?;
    Ok(())
}
