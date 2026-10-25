use crate::common::error::AppError;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    error::ErrorKind,
};
use sha2::{Digest, Sha256};

pub async fn migrate_rbac(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    let previous = metadata.find_one(doc! {"_id":"rbac"}).await?;
    if let Some(record) = &previous
        && !matches!(record.get_i32("version").ok(), Some(1 | 2))
    {
        return Err(AppError::SchemaMismatch);
    }
    if let Err(error) = database.create_collection("_invariants").await
        && !matches!(&*error.kind,ErrorKind::Command(command) if command.code==48)
    {
        return Err(error.into());
    }
    for key in ["rbac", "menus"] {
        database
            .collection::<Document>("_invariants")
            .update_one(doc! {"_id":key}, doc! {"$setOnInsert":{"version":1i64}})
            .upsert(true)
            .await?;
    }
    for keys in [
        doc! {"deletedAt":1,"createdAt":-1,"_id":-1},
        doc! {"deletedAt":1,"slug":1,"_id":1},
        doc! {"deletedAt":1,"name":1,"_id":1},
        doc! {"dataScopeRoleIds":1,"deletedAt":1},
    ] {
        database
            .collection::<Document>("roles")
            .create_index(IndexModel::builder().keys(keys).build())
            .await?;
    }
    database
        .collection::<Document>("users")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"deletedAt":1,"createdAt":-1,"_id":-1})
                .build(),
        )
        .await?;
    database
        .collection::<Document>("users")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"roleIds":1,"status":1,"deletedAt":1})
                .build(),
        )
        .await?;
    database
        .collection::<Document>("menus")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"deletedAt":1,"parentId":1,"sort":1,"_id":1})
                .build(),
        )
        .await?;
    if previous.is_none() {
        database
            .collection::<Document>("roles")
            .update_one(
                doc! {"slug":"owner"},
                doc! {"$set":{"dataScope":"all","dataScopeRoleIds":[]}},
            )
            .await?;
        let now = DateTime::now();
        for (path, label, view, permission, sort) in [
            ("/dashboard", "工作台", "dashboard", "dashboard.read", 0),
            ("/users", "用户管理", "users", "users.read", 10),
            ("/roles", "角色管理", "roles", "roles.read", 20),
            ("/menus", "菜单管理", "menus", "menus.read", 30),
            (
                "/permissions",
                "权限目录",
                "permissions",
                "permissions.read",
                40,
            ),
        ] {
            if database
                .collection::<Document>("menus")
                .find_one(doc! {"path":path,"deletedAt":null})
                .await?
                .is_some()
            {
                continue;
            }
            let digest = Sha256::digest(format!("tri-admin:menu:{path}"));
            let mut bytes = [0u8; 12];
            bytes.copy_from_slice(&digest[..12]);
            let id = ObjectId::from_bytes(bytes);
            database.collection::<Document>("menus").update_one(doc!{"_id":id},doc!{"$setOnInsert":{"label":label,"path":path,"viewKey":view,"permission":permission,"icon":null,"parentId":null,"sort":sort,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
        metadata
            .update_one(
                doc! {"_id":"rbac"},
                doc! {"$setOnInsert":{"version":2,"createdAt":now}},
            )
            .upsert(true)
            .await?;
    }
    if previous
        .as_ref()
        .is_some_and(|record| record.get_i32("version").ok() == Some(1))
    {
        database.collection::<Document>("roles")
            .update_many(doc!{"dataScopeRoleIds":{"$exists":false}},doc!{"$set":{"dataScopeRoleIds":[],"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).await?;
        metadata
            .update_one(
                doc! {"_id":"rbac","version":1},
                doc! {"$set":{"version":2,"updatedAt":DateTime::now()}},
            )
            .await?;
    }
    Ok(())
}
