use crate::common::error::AppError;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use sha2::{Digest, Sha256};
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    for (path, label, view, permission, sort) in [
        (
            "/api-tokens",
            "API Token",
            "tokens",
            "apiTokens.read",
            120i32,
        ),
        (
            "/applications",
            "开放应用",
            "apps",
            "applications.read",
            130i32,
        ),
    ] {
        let hash = Sha256::digest(format!("tri-admin:menu:{path}"));
        let mut bytes = [0; 12];
        bytes.copy_from_slice(&hash[..12]);
        let id = ObjectId::from_bytes(bytes);
        if database
            .collection::<Document>("menus")
            .find_one(doc! {"path":path,"deletedAt":null})
            .await?
            .is_none()
        {
            let now = DateTime::now();
            database.collection::<Document>("menus").update_one(doc!{"_id":id},doc!{"$setOnInsert":{"label":label,"path":path,"viewKey":view,"permission":permission,"icon":null,"parentId":null,"sort":sort,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}}).upsert(true).await?;
        }
    }
    Ok(())
}
