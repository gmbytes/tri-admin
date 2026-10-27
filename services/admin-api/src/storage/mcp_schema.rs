use crate::common::error::AppError;
use crate::common::transaction::{self, Failure, fail};
use futures_util::FutureExt;
use mongodb::{
    Database, IndexModel,
    bson::{DateTime, Document, doc},
    error::ErrorKind,
    options::IndexOptions,
};
pub const VERSION: i32 = 2;
pub async fn migrate(database: &Database) -> Result<(), AppError> {
    let metadata = database.collection::<Document>("_schema_migrations");
    if let Some(old) = metadata.find_one(doc! {"_id":"mcp"}).await?
        && !old.get_i32("version").is_ok_and(|v| v == 1 || v == VERSION)
    {
        return Err(AppError::SchemaMismatch);
    }
    for name in [
        "mcp_connections",
        "mcp_instances",
        "mcp_actions",
        "mcp_results",
    ] {
        if let Err(error) = database.create_collection(name).await
            && !matches!(&*error.kind,ErrorKind::Command(error)if error.code==48)
        {
            return Err(error.into());
        }
    }
    for (name, keys, unique, partial) in [
        (
            "mcp_connections",
            doc! {"ownerId":1,"requestId":1},
            true,
            None,
        ),
        (
            "mcp_connections",
            doc! {"ownerId":1,"name":1},
            true,
            Some(doc! {"deletedAt":null}),
        ),
        (
            "mcp_connections",
            doc! {"deletedAt":1,"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "mcp_connections",
            doc! {"transport.profileId":1,"deletedAt":1},
            false,
            None,
        ),
        (
            "mcp_instances",
            doc! {"connectionId":1,"createdAt":-1},
            false,
            None,
        ),
        (
            "mcp_instances",
            doc! {"releaseConfirmed":1,"leaseUntil":1},
            false,
            None,
        ),
        ("mcp_actions", doc! {"actorId":1,"requestId":1}, true, None),
        (
            "mcp_actions",
            doc! {"ownerId":1,"createdAt":-1,"_id":-1},
            false,
            None,
        ),
        (
            "mcp_actions",
            doc! {"connectionId":1,"status":1,"createdAt":1},
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
        .collection::<Document>("mcp_results")
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
        .update_one(doc! {"_id":"mcp"}, doc! {"$setOnInsert":{"version":0i64}})
        .upsert(true)
        .await?;
    transaction::run(database,database.clone(),|session,db|async move{
        crate::modules::rbac::shared::lock(db,"mcp",session).await?;
        let marker=db.collection::<Document>("_schema_migrations").find_one(doc!{"_id":"mcp"}).session(&mut *session).await?;
        let version=marker.as_ref().map(|row|row.get_i32("version")).transpose().map_err(|_|fail(Failure::NotReady))?;
        if version==Some(VERSION){return Ok(())}
        if version.is_some_and(|v|v!=1){return Err(fail(Failure::NotReady))}
        if version==Some(1){
            if db.collection::<Document>("mcp_connections").count_documents(doc!{"$or":[{"transport.kind":{"$ne":"stdio"}},{"runtime":{"$type":"object"},"runtime.exitConfirmed":{"$not":{"$type":"bool"}}}]}).session(&mut *session).await?>0
                || db.collection::<Document>("mcp_instances").count_documents(doc!{"exitConfirmed":{"$not":{"$type":"bool"}}}).session(&mut *session).await?>0{return Err(fail(Failure::NotReady))}
            db.collection::<Document>("mcp_connections").update_many(doc!{"runtime":{"$type":"object"}},doc!{"$rename":{"runtime.exitConfirmed":"runtime.releaseConfirmed"},"$set":{"runtime.transportKind":"stdio"}}).session(&mut *session).await?;
            db.collection::<Document>("mcp_instances").update_many(doc!{},doc!{"$rename":{"exitConfirmed":"releaseConfirmed"},"$set":{"transportKind":"stdio"}}).session(&mut *session).await?;
        }else{
            for collection in ["mcp_connections","mcp_instances","mcp_actions","mcp_results"]{if db.collection::<Document>(collection).count_documents(doc!{}).session(&mut *session).await?>0{return Err(fail(Failure::NotReady))}}
        }
        db.collection::<Document>("_schema_migrations").update_one(doc!{"_id":"mcp"},doc!{"$set":{"version":VERSION,"updatedAt":DateTime::now()},"$setOnInsert":{"createdAt":DateTime::now()}}).upsert(true).session(session).await?;
        Ok(())
    }.boxed()).await?;
    if let Err(error) = database
        .collection::<Document>("mcp_instances")
        .drop_index("exitConfirmed_1_leaseUntil_1")
        .await
        && !matches!(&*error.kind,ErrorKind::Command(error)if error.code==27)
    {
        return Err(error.into());
    }
    Ok(())
}
