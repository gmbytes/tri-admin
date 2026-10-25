use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
use sha2::{Digest, Sha256};

const DESCRIPTOR: &str = "tri-admin/core/v1:users(usernameKey active unique),roles(slug unique),menus(path active unique),sessions(refreshHash unique,expiresAt ttl)";

pub async fn is_initialized(database: &Database) -> Result<bool, AppError> {
    let count = database
        .collection::<Document>("_schema_migrations")
        .count_documents(doc! {"$or":[
            {"_id":"core","version":1,"bootstrapComplete":true},
            {"_id":"auth","version":1},
            {"_id":"rbac","version":2},
            {"_id":"dictionaries","version":1},
            {"_id":"parameters","version":1},
            {"_id":"versions","version":1},
            {"_id":"version-imports","version":1}
        ]})
        .await?;
    Ok(count == 7)
}

pub async fn migrate_database(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let checksum = format!("{:x}", Sha256::digest(DESCRIPTOR));
    if let Some(record) = metadata.find_one(doc! {"_id":"core"}).await?
        && (record.get_i32("version").ok() != Some(1)
            || record.get_str("checksum").ok() != Some(checksum.as_str()))
    {
        return Err(AppError::SchemaMismatch);
    }
    let hello = database.run_command(doc! {"hello":1}).await?;
    if !hello.contains_key("setName") && hello.get_str("msg").ok() != Some("isdbgrid") {
        return Err(AppError::Configuration(
            "MongoDB 需要副本集以支持事务".into(),
        ));
    }
    for name in ["users", "roles", "menus", "sessions", "_schema_migrations"] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
        {
            return Err(error.into());
        }
    }
    database
        .collection::<Document>("users")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"usernameKey":1})
                .options(
                    IndexOptions::builder()
                        .name("active_username_unique".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! {"deletedAt":null})
                        .build(),
                )
                .build(),
        )
        .await?;
    database
        .collection::<Document>("roles")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"slug":1})
                .options(
                    IndexOptions::builder()
                        .name("role_slug_unique".to_owned())
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await?;
    database
        .collection::<Document>("menus")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"path":1})
                .options(
                    IndexOptions::builder()
                        .name("active_menu_path_unique".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! {"deletedAt":null})
                        .build(),
                )
                .build(),
        )
        .await?;
    database
        .collection::<Document>("sessions")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"refreshHash":1})
                .options(
                    IndexOptions::builder()
                        .name("refresh_hash_unique".to_owned())
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await?;
    database
        .collection::<Document>("sessions")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"expiresAt":1})
                .options(
                    IndexOptions::builder()
                        .name("session_expiry".to_owned())
                        .expire_after(std::time::Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await?;
    metadata.update_one(doc!{"_id":"core"},doc!{"$setOnInsert":{"version":1,"checksum":checksum,"createdAt":DateTime::now(),"bootstrapComplete":false}}).upsert(true).await?;
    super::auth_schema::migrate_auth(database).await?;
    super::rbac_schema::migrate_rbac(database).await?;
    super::dictionary_schema::migrate_dictionaries(database).await?;
    super::parameter_schema::migrate_parameters(database).await?;
    super::version_schema::migrate_versions(database).await?;
    super::version_import_schema::migrate_imports(database).await
}
