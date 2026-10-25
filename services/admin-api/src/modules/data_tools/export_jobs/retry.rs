use super::super::{models::public_time, registry};
use super::{
    journal::Attempt,
    models::{ACTIVE, JobRecord, JobState, JobView},
    service,
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared::{self, Actor},
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::{Deserialize, Serialize};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryInput {
    pub request_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Action {
    job_id: ObjectId,
    actor_id: ObjectId,
    request_id: String,
    kind: String,
    generation: i64,
    created_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    job_id: String,
    request_id: String,
    kind: String,
    generation: i64,
    created_at: String,
}
fn canonical(value: &str) -> Result<(), AppError> {
    if uuid::Uuid::parse_str(value)
        .ok()
        .is_none_or(|id| id.to_string() != value)
    {
        return Err(AppError::Validation("重试操作标识需为规范 UUID".into()));
    }
    Ok(())
}
pub async fn receipt(
    database: &Database,
    actor: &Actor,
    id: ObjectId,
    request: &str,
) -> Result<ActionView, AppError> {
    canonical(request)?;
    service::get_record(database, actor, id).await?;
    let action = database
        .collection::<Action>("data_export_actions")
        .find_one(doc! {"jobId":id,"actorId":actor.id,"requestId":request})
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(ActionView {
        job_id: id.to_hex(),
        request_id: action.request_id,
        kind: action.kind,
        generation: action.generation,
        created_at: public_time(action.created_at)?,
    })
}
pub async fn request(
    database: &Database,
    actor: Actor,
    id: ObjectId,
    input: RetryInput,
) -> Result<(bool, JobView), AppError> {
    canonical(&input.request_id)?;
    let (created,row)=transaction::run(database,(database.clone(),actor,input),|session,(database,actor,input)|async move{
        shared::lock(database,"rbac",session).await?;shared::lock(database,"data-exports",session).await?;
        let mut job=database.collection::<JobRecord>("data_exports").find_one(doc!{"_id":id,"actorId":actor.id}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        registry::resource(&job.meta.resource,actor,false).map_err(|_|fail(Failure::Forbidden))?;
        if database.collection::<Action>("data_export_actions").find_one(doc!{"jobId":id,"actorId":actor.id,"requestId":&input.request_id}).session(&mut *session).await?.is_some(){return Ok((false,job));}
        if !matches!(job.meta.state,JobState::Failed|JobState::Unknown){return Err(fail(Failure::Conflict("当前任务无需重试")));}
        if database.collection::<Document>("data_export_actions").count_documents(doc!{"jobId":id}).session(&mut *session).await?>=128{return Err(fail(Failure::RateLimited));}
        let candidate=if let Some(candidate)=job.candidate_id {database.collection::<Attempt>("data_export_attempts").find_one(doc!{"_id":candidate}).session(&mut *session).await?}else{None};
        if candidate.as_ref().is_some_and(|candidate|candidate.cleanup_expires_at.is_some_and(|expires|expires>DateTime::now())||candidate.state=="deleting"){return Err(fail(Failure::Conflict("候选正在清理，请查询后重试")));}
        let written=candidate.as_ref().is_some_and(|candidate|candidate.write_started&&candidate.state!="cleaned");
        if job.meta.state==JobState::Failed&&(database.collection::<Document>("data_exports").count_documents(doc!{"state":{"$in":ACTIVE.to_vec()}}).session(&mut *session).await?>=8||database.collection::<Document>("data_exports").count_documents(doc!{"actorId":actor.id,"state":{"$in":ACTIVE.to_vec()}}).session(&mut *session).await?>=2){return Err(fail(Failure::RateLimited));}
        if (job.meta.state==JobState::Failed&&!written)||candidate.as_ref().is_some_and(|candidate|candidate.state=="cleaned") {
            if let Some(candidate)=job.candidate_id {database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":candidate,"state":{"$ne":"cleaned"}},doc!{"$set":{"state":"orphaned","retryRequested":false,"nextRunAt":DateTime::now()}}).session(&mut *session).await?;}
            job.meta.state=JobState::Queued;job.candidate_id=None;job.meta.row_count=0;job.meta.output=None;job.meta.started_at=None;job.meta.completed_at=None;job.meta.expires_at=None;job.lease_token=None;job.lease_expires_at=None;
        } else {
            job.meta.state=JobState::Unknown;
            let candidate=job.candidate_id.ok_or_else(||fail(Failure::NotReady))?;
            database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":candidate},doc!{"$set":{"retryRequested":true}}).session(&mut *session).await?;
        }
        let now=DateTime::now();job.meta.last_error=None;job.next_run_at=now;job.meta.updated_at=now;job.meta.version+=1;job.storage_retained=true;job.meta.cleanup_pending=true;
        database.collection::<Document>("data_exports").replace_one(doc!{"_id":id},mongodb::bson::serialize_to_document(&job).map_err(|_|fail(Failure::NotReady))?).session(&mut *session).await?;
        database.collection::<Action>("data_export_actions").insert_one(Action{job_id:id,actor_id:actor.id,request_id:input.request_id.clone(),kind:"retry".into(),generation:job.generation,created_at:now}).session(&mut *session).await?;
        shared::audit(database,actor,"dataTools.export.retry",id,session).await?;Ok((true,job))
    }.boxed()).await?;
    Ok((created, row.meta.view(DateTime::now())?))
}
