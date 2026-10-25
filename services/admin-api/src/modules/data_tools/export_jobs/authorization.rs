use super::super::registry;
use super::models::JobRecord;
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        transaction::{self, Failure, fail},
    },
    modules::{
        auth::repository,
        rbac::shared::{self, Actor},
    },
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestBatch {
    job_id: ObjectId,
    generation: i64,
    batch: u32,
    row_ids: Vec<ObjectId>,
}

/// Background execution uses the active owner's latest role summary, without
/// borrowing a browser session or reading credential fields into this path.
pub async fn execution_actor(
    database: &Database,
    owner: ObjectId,
    resource: &str,
) -> Result<Actor, AppError> {
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {"_id":owner,"status":"active","deletedAt":null})
        .projection(doc! {"username":1,"displayName":1,"roleIds":1})
        .await?
        .ok_or(AppError::Forbidden)?;
    let actor = Actor::from_summary(&repository::summary(database, &user).await?)?;
    if !actor.can("exports.write") {
        return Err(AppError::Forbidden);
    }
    registry::resource(resource, &actor, false)?;
    Ok(actor)
}
pub fn user_filter(job: &JobRecord, actor: &Actor) -> Document {
    let captured = match job.creation_scope.scope {
        DataScope::All => doc! {},
        DataScope::Own => doc! {"_id":job.actor_id},
        DataScope::Roles => {
            doc! {"$or":[{"_id":job.actor_id},{"roleIds":{"$in":&job.creation_scope.scope_role_ids}}]}
        }
    };
    doc! {"$and":[captured,actor.user_filter()]}
}
pub async fn record_manifest_batch(
    database: &Database,
    job_id: ObjectId,
    generation: i64,
    token: &str,
    batch: u32,
    row_ids: Vec<ObjectId>,
) -> Result<bool, AppError> {
    if batch >= 1000
        || row_ids.is_empty()
        || row_ids.len() > 1000
        || row_ids.iter().collect::<HashSet<_>>().len() != row_ids.len()
    {
        return Err(AppError::Validation("导出行清单批次无效".into()));
    }
    transaction::run(database,(database.clone(),token.to_owned(),row_ids),|session,(database,token,row_ids)|async move{
        shared::lock(database,"data-exports",session).await?;
        let now=DateTime::now();
        let row=database.collection::<JobRecord>("data_exports").find_one(doc!{"_id":job_id,"generation":generation,"state":"running","leaseToken":&*token,"leaseExpiresAt":{"$gt":now}}).session(&mut *session).await?.ok_or_else(||fail(Failure::Conflict("导出执行租约已失效")))?;
        if row.meta.resource!="users"{return Err(fail(Failure::Validation("该资源无需用户行清单")));}
        let filter=doc!{"jobId":job_id,"generation":generation,"batch":batch as i64};
        if let Some(existing)=database.collection::<ManifestBatch>("data_export_manifest").find_one(filter).session(&mut *session).await? {
            if existing.row_ids!=*row_ids{return Err(fail(Failure::Conflict("行清单批次已用于不同内容")));}return Ok(false);
        }
        let expected=database.collection::<Document>("data_export_manifest").count_documents(doc!{"jobId":job_id,"generation":generation}).session(&mut *session).await?;
        if expected!=batch as u64{return Err(fail(Failure::Validation("行清单批次必须连续")));}
        let total=row.meta.row_count.checked_add(row_ids.len() as u32).filter(|count|*count<=row.meta.max_rows).ok_or_else(||fail(Failure::Validation("导出超过本次行数上限")))?;
        database.collection::<ManifestBatch>("data_export_manifest").insert_one(ManifestBatch{job_id,generation,batch,row_ids:row_ids.clone()}).session(&mut *session).await?;
        database.collection::<Document>("data_exports").update_one(doc!{"_id":job_id},doc!{"$set":{"rowCount":total as i64,"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;
        Ok(true)
    }.boxed()).await
}

/// Verifies only row authorization. The caller must separately enforce current
/// job state, artifact expiry and its own read/write permission before publishing
/// or sending any content bytes.
pub async fn verify_user_manifest(
    database: &Database,
    job: &JobRecord,
    actor: &Actor,
) -> Result<(), AppError> {
    if job.actor_id != actor.id {
        return Err(AppError::NotFound);
    }
    registry::resource(&job.meta.resource, actor, false)?;
    if job.meta.resource != "users" {
        return Ok(());
    }
    transaction::run_snapshot(database,(database.clone(),actor.user_filter(),job.meta.id,job.generation,job.meta.row_count),|session,(database,scope,id,generation,expected)|async move{
        let mut cursor=database.collection::<ManifestBatch>("data_export_manifest").find(doc!{"jobId":*id,"generation":*generation}).sort(doc!{"batch":1}).batch_size(8).session(&mut *session).await?;
        let mut index=0u32;let mut count=0u32;
        while let Some(batch)=cursor.next(&mut *session).await.transpose()? {
            if batch.batch!=index||index>=1000||batch.row_ids.is_empty()||batch.row_ids.len()>1000||batch.row_ids.iter().collect::<HashSet<_>>().len()!=batch.row_ids.len(){return Err(fail(Failure::NotReady));}
            let readable=database.collection::<Document>("users").count_documents(doc!{"$and":[{"_id":{"$in":&batch.row_ids}},{"deletedAt":null},scope.clone()]}).session(&mut *session).await?;
            if readable!=batch.row_ids.len() as u64{return Err(fail(Failure::Conflict("导出数据范围已变化，请重新导出")));}
            count=count.checked_add(batch.row_ids.len() as u32).ok_or_else(||fail(Failure::NotReady))?;index+=1;
        }
        if count!=*expected{return Err(fail(Failure::NotReady));}Ok(())
    }.boxed()).await
}
