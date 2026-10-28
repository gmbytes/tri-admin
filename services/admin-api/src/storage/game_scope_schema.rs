use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::rbac::{permissions, shared},
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc},
};
pub const VERSION: i32 = 1;
pub async fn migrate(db: &Database) -> Result<(), AppError> {
    if let Some(row) = db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"game-scope"})
        .await?
        && row.get_i32("version").ok() != Some(VERSION)
    {
        return Err(AppError::SchemaMismatch);
    }
    transaction::run(db,db.clone(),|session,db|async move{
  shared::lock(db,"rbac",session).await?;
  let marker=db.collection::<Document>("_schema_migrations").find_one(doc!{"_id":"game-scope"}).session(&mut *session).await?;
  if let Some(marker)=marker{if marker.get_i32("version").ok()!=Some(VERSION){return Err(fail(Failure::NotReady))}return Ok(())}
  let now=DateTime::now();
  let views=["dictionaries","parameters","settings","exports","tokens","apps","codegen","skills","mcp","ai","workflows","versions","customers","clients"];
  let paths=["/dictionaries","/parameters","/settings","/data-tools","/api-tokens","/applications","/codegen","/skills","/mcp","/ai","/workflows","/versions","/customers","/clients"];
  db.collection::<Document>("menus").update_many(doc!{"deletedAt":null,"$or":[{"viewKey":{"$in":views}},{"path":{"$in":paths}}]},doc!{"$set":{"enabled":false,"deletedAt":now,"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;
  let mut allowed=vec!["*"];allowed.extend(permissions::CATALOG.iter().map(|permission|permission.code));
  db.collection::<Document>("roles").update_many(doc!{"permissions":{"$elemMatch":{"$nin":&allowed}}},doc!{"$pull":{"permissions":{"$nin":&allowed}},"$set":{"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;
  db.collection::<Document>("_schema_migrations").insert_one(doc!{"_id":"game-scope","version":VERSION,"createdAt":now}).session(session).await?;
  Ok(())
 }.boxed()).await
}
