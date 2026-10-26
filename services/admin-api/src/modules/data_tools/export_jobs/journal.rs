use super::models::{JobRecord, JobState, Output};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared::{self, Actor},
    state::AppState,
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub job_id: ObjectId,
    pub generation: i64,
    pub workspace_id: String,
    pub blob_id: ObjectId,
    pub storage_profile_id: ObjectId,
    pub state: String,
    pub size: Option<i64>,
    pub sha256: Option<String>,
    pub row_count: u32,
    pub write_started: bool,
    pub retry_requested: bool,
    pub blob_known: bool,
    pub workspace_cleaned: bool,
    pub cleanup_token: Option<String>,
    pub cleanup_expires_at: Option<DateTime>,
    pub next_run_at: DateTime,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
#[derive(Clone)]
pub struct Claim {
    pub job: JobRecord,
    pub attempt: Attempt,
    pub token: String,
    pub fresh: bool,
}
pub fn lease(claim: &Claim) -> Document {
    doc! {"_id":claim.job.meta.id,"generation":claim.job.generation,"leaseToken":&claim.token,"leaseExpiresAt":{"$gt":DateTime::now()},"state":{"$in":["running","publishing","reconciling"]}}
}
pub async fn next(database: &Database) -> Result<Option<JobRecord>, AppError> {
    let now = DateTime::now();
    Ok(database.collection::<JobRecord>("data_exports").find_one(doc!{"state":{"$in":["queued","running","publishing","reconciling","unknown"]},"nextRunAt":{"$lte":now},"$or":[{"leaseExpiresAt":null},{"leaseExpiresAt":{"$lte":now}}]}).sort(doc!{"createdAt":1,"_id":1}).await?)
}
pub async fn claim(
    state: &AppState,
    id: ObjectId,
    workspace: Option<String>,
) -> Result<Option<Claim>, AppError> {
    let database = state.require_database()?;
    let attempt_id = ObjectId::new();
    let blob = ObjectId::new();
    let token = uuid::Uuid::new_v4().to_string();
    transaction::run(database,(database.clone(),workspace,token),|session,(database,workspace,token)|async move{
        shared::lock(database,"data-exports",session).await?;let now=DateTime::now();
        if database.collection::<Document>("data_exports").count_documents(doc!{"leaseExpiresAt":{"$gt":now}}).session(&mut *session).await?>=2{return Ok(None);}
        let Some(mut job)=database.collection::<JobRecord>("data_exports").find_one(doc!{"_id":id,"state":{"$in":["queued","running","publishing","reconciling","unknown"]},"nextRunAt":{"$lte":now},"$or":[{"leaseExpiresAt":null},{"leaseExpiresAt":{"$lte":now}}]}).session(&mut *session).await? else{return Ok(None)};
        let fresh=job.meta.state==JobState::Queued;
        let attempt=if fresh {
            let Some(workspace)=workspace.as_ref() else{return Ok(None)};
            job.generation+=1;job.meta.row_count=0;job.meta.output=None;job.meta.started_at=Some(now);job.meta.last_error=None;job.meta.state=JobState::Running;job.candidate_id=Some(attempt_id);
            let attempt=Attempt{id:attempt_id,job_id:id,generation:job.generation,workspace_id:workspace.clone(),blob_id:blob,storage_profile_id:job.storage_profile_id,state:"generating".into(),size:None,sha256:None,row_count:0,write_started:false,retry_requested:false,blob_known:false,workspace_cleaned:false,cleanup_token:None,cleanup_expires_at:None,next_run_at:now,created_at:now,updated_at:now};
            database.collection::<Attempt>("data_export_attempts").insert_one(&attempt).session(&mut *session).await?;attempt
        } else {
            let attempt=database.collection::<Attempt>("data_export_attempts").find_one(doc!{"_id":job.candidate_id.ok_or_else(||fail(Failure::NotReady))?}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotReady))?;
            if attempt.cleanup_expires_at.is_some_and(|expires|expires>now){return Ok(None);}
            job.meta.state=JobState::Reconciling;attempt
        };
        job.lease_token=Some(token.clone());job.lease_expires_at=Some(DateTime::from_millis(now.timestamp_millis()+60000));job.meta.updated_at=now;job.meta.version+=1;
        let document=mongodb::bson::serialize_to_document(&job).map_err(|_|fail(Failure::NotReady))?;
        database.collection::<Document>("data_exports").replace_one(doc!{"_id":id},document).session(session).await?;
        Ok(Some(Claim{job,attempt,token:token.clone(),fresh}))
    }.boxed()).await
}
pub async fn renew(database: &Database, claim: &Claim) -> Result<bool, AppError> {
    Ok(database.collection::<Document>("data_exports").update_one(lease(claim),doc!{"$set":{"leaseExpiresAt":DateTime::from_millis(DateTime::now().timestamp_millis()+60000)}}).await?.matched_count==1)
}
pub async fn progress(database: &Database, claim: &Claim, count: u32) -> Result<(), AppError> {
    if count > claim.job.meta.max_rows {
        return Err(AppError::Validation("ROW_LIMIT".into()));
    }
    let changed=database.collection::<Document>("data_exports").update_one(lease(claim),doc!{"$set":{"rowCount":count as i64,"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).await?;
    if changed.matched_count != 1 {
        return Err(AppError::Conflict("LEASE_LOST".into()));
    }
    Ok(())
}
pub async fn prepared(
    database: &Database,
    claim: &Claim,
    size: u64,
    sha: &str,
    rows: u32,
) -> Result<(), AppError> {
    transaction::run(database,(database.clone(),claim.clone(),sha.to_owned()),|session,(database,claim,sha)|async move{
        shared::lock(database,"data-exports",session).await?;
        if database.collection::<Document>("data_exports").update_one(lease(claim),doc!{"$set":{"rowCount":rows as i64,"state":"publishing","updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?.matched_count!=1{return Err(fail(Failure::Conflict("LEASE_LOST")));}
        database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":claim.attempt.id,"state":"generating"},doc!{"$set":{"state":"prepared","size":size as i64,"sha256":&*sha,"rowCount":rows as i64,"updatedAt":DateTime::now()}}).session(session).await?;Ok(())
    }.boxed()).await
}
pub async fn putting(database: &Database, claim: &Claim) -> Result<(), AppError> {
    transaction::run(database,(database.clone(),claim.clone()),|session,(database,claim)|async move{
        shared::lock(database,"data-exports",session).await?;
        if database.collection::<Document>("data_exports").find_one(lease(claim)).session(&mut *session).await?.is_none(){return Err(fail(Failure::Conflict("LEASE_LOST")));}
        let changed=database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":claim.attempt.id,"writeStarted":false,"state":"prepared"},doc!{"$set":{"state":"putting","writeStarted":true,"updatedAt":DateTime::now()}}).session(session).await?;
        if changed.matched_count!=1{return Err(fail(Failure::Conflict("写入候选必须先确认")));}Ok(())
    }.boxed()).await
}
pub async fn unknown(database: &Database, claim: &Claim, code: &str) -> Result<(), AppError> {
    database.collection::<Document>("data_exports").update_one(doc!{"_id":claim.job.meta.id,"generation":claim.job.generation,"leaseToken":&claim.token},doc!{"$set":{"state":"unknown","lastError":code,"leaseToken":null,"leaseExpiresAt":null,"cleanupPending":true,"nextRunAt":DateTime::from_millis(DateTime::now().timestamp_millis()+30000),"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).await?;
    Ok(())
}
pub async fn rejected(database: &Database, claim: &Claim) -> Result<(), AppError> {
    transaction::run(database, (database.clone(), claim.clone()), |session, (database, claim)| async move {
        shared::lock(database, "data-exports", session).await?;
        let changed = database.collection::<Document>("data_export_attempts").update_one(
            doc! {"_id":claim.attempt.id,"jobId":claim.job.meta.id,"generation":claim.job.generation,"state":"putting","writeStarted":true,"blobKnown":false},
            doc! {"$set":{"writeStarted":false,"state":"orphaned","updatedAt":DateTime::now()}},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 { return Err(fail(Failure::Conflict("写入候选已改变"))); }
        // Cancellation may already have won the job fence. The rejected candidate
        // still has no object, but must never overwrite that terminal decision.
        database.collection::<Document>("data_exports").update_one(
            doc! {"_id":claim.job.meta.id,"generation":claim.job.generation,"leaseToken":&claim.token},
            doc! {"$set":{"state":"failed","lastError":"STORAGE_WRITE_REJECTED","leaseToken":null,"leaseExpiresAt":null,"cleanupPending":true,"updatedAt":DateTime::now()},"$inc":{"version":1i64}},
        ).session(session).await?;
        Ok(())
    }.boxed()).await
}
pub async fn failed(database: &Database, claim: &Claim, code: &str) -> Result<(), AppError> {
    database.collection::<Document>("data_exports").update_one(doc!{"_id":claim.job.meta.id,"generation":claim.job.generation,"leaseToken":&claim.token},doc!{"$set":{"state":"failed","lastError":code,"leaseToken":null,"leaseExpiresAt":null,"cleanupPending":true,"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).await?;
    Ok(())
}
pub async fn requeue(database: &Database, claim: &Claim) -> Result<(), AppError> {
    transaction::run(database,(database.clone(),claim.clone()),|session,(database,claim)|async move{
        shared::lock(database,"data-exports",session).await?;
        if database.collection::<Document>("data_exports").update_one(lease(claim),doc!{"$set":{"state":"queued","candidateId":null,"leaseToken":null,"leaseExpiresAt":null,"nextRunAt":DateTime::now(),"cleanupPending":true},"$inc":{"version":1i64}}).session(&mut *session).await?.matched_count==1 {
            database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":claim.attempt.id,"writeStarted":false,"state":{"$ne":"cleaned"}},doc!{"$set":{"state":"orphaned"}}).session(session).await?;
        }Ok(())
    }.boxed()).await
}
pub async fn complete(
    database: &Database,
    claim: &Claim,
    actor: Actor,
    attempt: &Attempt,
) -> Result<(), AppError> {
    let output = Output {
        blob_id: attempt.blob_id,
        size: attempt.size.ok_or(AppError::NotReady)?,
        sha256: attempt.sha256.clone().ok_or(AppError::NotReady)?,
    };
    transaction::run(database,(database.clone(),claim.clone(),actor,output,attempt.row_count),|session,(database,claim,actor,output,rows)|async move{
        shared::lock(database,"rbac",session).await?;shared::lock(database,"data-exports",session).await?;
        let now=DateTime::now();let data=mongodb::bson::serialize_to_document(&output).map_err(|_|fail(Failure::NotReady))?;
        if database.collection::<Document>("data_exports").update_one(lease(claim),doc!{"$set":{"state":"completed","output":data,"rowCount":*rows as i64,"completedAt":now,"expiresAt":DateTime::from_millis(now.timestamp_millis()+86400000),"leaseToken":null,"leaseExpiresAt":null,"lastError":null,"cleanupPending":true,"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?.matched_count!=1{return Err(fail(Failure::Conflict("LEASE_LOST")));}
        database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":claim.attempt.id},doc!{"$set":{"state":"published","updatedAt":now}}).session(&mut *session).await?;
        shared::audit(database,actor,"dataTools.export.complete",claim.job.meta.id,session).await?;Ok(())
    }.boxed()).await
}
pub async fn current(database: &Database, claim: &Claim) -> Result<JobRecord, AppError> {
    database
        .collection::<JobRecord>("data_exports")
        .find_one(doc! {"_id":claim.job.meta.id})
        .await?
        .ok_or(AppError::NotFound)
}
pub async fn supersede(database: &Database, claim: &Claim) -> Result<(), AppError> {
    transaction::run(database,(database.clone(),claim.clone()),|session,(database,claim)|async move{
        shared::lock(database,"data-exports",session).await?;
        let candidate=database.collection::<Attempt>("data_export_attempts").find_one(doc!{"_id":claim.attempt.id,"retryRequested":true,"state":{"$ne":"cleaned"}}).session(&mut *session).await?.ok_or_else(||fail(Failure::Conflict("重试意图已改变")))?;
        if database.collection::<Document>("data_exports").update_one(lease(claim),doc!{"$set":{"state":"queued","candidateId":null,"rowCount":0i64,"output":null,"leaseToken":null,"leaseExpiresAt":null,"nextRunAt":DateTime::now(),"cleanupPending":true,"lastError":null},"$inc":{"version":1i64}}).session(&mut *session).await?.matched_count!=1{return Err(fail(Failure::Conflict("LEASE_LOST")));}
        database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":candidate.id},doc!{"$set":{"state":"orphaned","retryRequested":false,"nextRunAt":DateTime::now()}}).session(session).await?;Ok(())
    }.boxed()).await
}
pub async fn attempt(database: &Database, id: ObjectId) -> Result<Attempt, AppError> {
    database
        .collection::<Attempt>("data_export_attempts")
        .find_one(doc! {"_id":id})
        .await?
        .ok_or(AppError::NotReady)
}
pub async fn workspace_cleaned(database: &Database, claim: &Claim) -> Result<(), AppError> {
    workspace_cleaned_by_id(database, claim.job.meta.id, claim.attempt.id).await
}
pub async fn workspace_cleaned_by_id(
    database: &Database,
    job_id: ObjectId,
    attempt_id: ObjectId,
) -> Result<(), AppError> {
    transaction::run(database,database.clone(),|session,database|async move{
        shared::lock(database,"data-exports",session).await?;
        let attempt=database.collection::<Attempt>("data_export_attempts").find_one(doc!{"_id":attempt_id}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotReady))?;
        database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":attempt.id},doc!{"$set":{"workspaceCleaned":true}}).session(&mut *session).await?;
        let job=database.collection::<JobRecord>("data_exports").find_one(doc!{"_id":job_id}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        if !attempt.write_started&&attempt.state!="cleaned"&&(!matches!(job.meta.state,JobState::Running|JobState::Publishing|JobState::Reconciling)||job.generation!=attempt.generation){database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":attempt.id},doc!{"$set":{"state":"cleaned"}}).session(&mut *session).await?;database.collection::<Document>("data_export_manifest").delete_many(doc!{"jobId":job.meta.id,"generation":attempt.generation}).session(&mut *session).await?;
            database.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"administration","action":"dataTools.export.cleanup","actorId":null,"subjectOwnerId":job.actor_id,"targetId":job.meta.id,"outcome":"success","createdAt":DateTime::now()})).session(&mut *session).await?;
        }
        if attempt.state=="published"&&job.meta.state==JobState::Completed&&let Some(expires)=job.meta.expires_at {database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":attempt.id},doc!{"$set":{"nextRunAt":expires}}).session(&mut *session).await?;}
        let pending=database.collection::<Document>("data_export_attempts").count_documents(doc!{"jobId":job.meta.id,"$or":[{"workspaceCleaned":false},{"state":{"$nin":["published","cleaned"]}}]}).session(&mut *session).await?>0;
        let retained=job.meta.output.is_some()||database.collection::<Document>("data_export_attempts").count_documents(doc!{"jobId":job.meta.id,"state":{"$ne":"cleaned"}}).session(&mut *session).await?>0||matches!(job.meta.state,JobState::Queued|JobState::Running|JobState::Publishing|JobState::Reconciling|JobState::Unknown);
        database.collection::<Document>("data_exports").update_one(doc!{"_id":job.meta.id},doc!{"$set":{"cleanupPending":pending,"storageRetained":retained}}).session(session).await?;Ok(())
    }.boxed()).await
}
