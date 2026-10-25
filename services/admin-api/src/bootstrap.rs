use crate::{common::error::AppError, storage::schema::migrate_database};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
    options::ReturnDocument,
};
use serde::Serialize;

pub struct AdminInput {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapReport {
    pub admin_created: bool,
    pub user_id: String,
}

pub async fn bootstrap_database(
    database: &Database,
    input: &AdminInput,
) -> Result<BootstrapReport, AppError> {
    crate::common::validation::username(&input.username)?;
    crate::common::validation::password(&input.password)?;
    migrate_database(database).await?;
    let hash = crate::common::password::hash(input.password.clone()).await?;
    let mut session = database.client().start_session().await?;
    let result = session.start_transaction().and_run((database.clone(),input.username.clone(),hash),|session,(database,username,hash)| async move {
        let users = database.collection::<Document>("users");
        let key = username.to_lowercase();
        if let Some(existing) = users.find_one(doc!{"usernameKey":&key}).session(&mut *session).await? {
            let active = existing.get_str("status").ok()==Some("active") && existing.get("deletedAt").is_none_or(|value|matches!(value,mongodb::bson::Bson::Null));
            if !active { return Err(mongodb::error::Error::custom("existing-administrator-unavailable")); }
            let id=existing.get_object_id("_id").map_err(mongodb::error::Error::custom)?;
            database.collection::<Document>("_schema_migrations").update_one(doc!{"_id":"core"},doc!{"$set":{"bootstrapComplete":true}}).session(&mut *session).await?;
            return Ok(BootstrapReport{admin_created:false,user_id:id.to_hex()});
        }
        let now=DateTime::now();
        let owner=database.collection::<Document>("roles").find_one_and_update(doc!{"slug":"owner"},doc!{"$setOnInsert":{"name":"系统管理员","slug":"owner","permissions":["*"],"dataScope":"all","dataScopeRoleIds":[],"protected":true,"createdAt":now,"updatedAt":now,"version":1}})
            .upsert(true).return_document(ReturnDocument::After).session(&mut *session).await?
            .ok_or_else(||mongodb::error::Error::custom("owner-role-missing"))?;
        let role_id=owner.get_object_id("_id").map_err(mongodb::error::Error::custom)?;
        let user_id=ObjectId::new();
        users.insert_one(doc!{"_id":user_id,"username":username.as_str(),"usernameKey":key,"passwordHash":hash.as_str(),"displayName":"管理员","roleIds":[role_id],"status":"active","deletedAt":null,"createdAt":now,"updatedAt":now,"version":1})
            .session(&mut *session).await?;
        database.collection::<Document>("_schema_migrations").update_one(doc!{"_id":"core"},doc!{"$set":{"bootstrapComplete":true}}).session(&mut *session).await?;
        Ok(BootstrapReport{admin_created:true,user_id:user_id.to_hex()})
    }.boxed()).await?;
    Ok(result)
}
