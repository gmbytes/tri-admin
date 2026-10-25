use super::{
    journal::{self, Attempt},
    models::{JobRecord, JobState},
    workspace::Workspace,
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::{
        files::storage::{BlobId, BlobSpec, BlobStore, StorageError},
        rbac::shared,
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::Serialize;
#[derive(Default, Serialize)]
pub struct Report {
    pub scanned: u32,
    pub retired: u32,
    pub pending: u32,
}
fn terminal(job: &JobRecord, now: DateTime) -> bool {
    matches!(
        job.meta.state,
        JobState::Cancelled | JobState::Failed | JobState::Expired
    ) || (job.meta.state == JobState::Completed
        && job.meta.expires_at.is_some_and(|expires| expires <= now))
}
async fn release(database: &Database, id: ObjectId, token: &str) -> Result<(), AppError> {
    database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":id,"cleanupToken":token},doc!{"$set":{"cleanupToken":null,"cleanupExpiresAt":null,"nextRunAt":DateTime::from_millis(DateTime::now().timestamp_millis()+30000)}}).await?;
    Ok(())
}
async fn finish(database: &Database, attempt: &Attempt, token: &str) -> Result<bool, AppError> {
    transaction::run(database,(database.clone(),attempt.clone(),token.to_owned()),|session,(database,attempt,token)|async move{
        shared::lock(database,"data-exports",session).await?;
        let changed=database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":attempt.id,"state":{"$ne":"cleaned"},"cleanupToken":&*token,"cleanupExpiresAt":{"$gt":DateTime::now()}},doc!{"$set":{"state":"cleaned","workspaceCleaned":true,"cleanupToken":null,"cleanupExpiresAt":null,"updatedAt":DateTime::now()}}).session(&mut *session).await?;
        if changed.matched_count==0{return Ok(false);}
        let job=database.collection::<JobRecord>("data_exports").find_one(doc!{"_id":attempt.job_id}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotFound))?;
        database.collection::<Document>("data_export_manifest").delete_many(doc!{"jobId":attempt.job_id,"generation":attempt.generation}).session(&mut *session).await?;
        let remaining=database.collection::<Document>("data_export_attempts").count_documents(doc!{"jobId":attempt.job_id,"state":{"$ne":"cleaned"}}).session(&mut *session).await?;
        if remaining==0&&terminal(&job,DateTime::now()) {let mut update=doc!{"output":null,"cleanupPending":false,"storageRetained":false};if job.meta.state==JobState::Completed{update.insert("state","expired");}database.collection::<Document>("data_exports").update_one(doc!{"_id":attempt.job_id},doc!{"$set":update}).session(&mut *session).await?;}
        database.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"administration","action":"dataTools.export.cleanup","actorId":null,"subjectOwnerId":job.actor_id,"targetId":job.meta.id,"outcome":"success","createdAt":DateTime::now()})).session(session).await?;Ok(true)
    }.boxed()).await
}
async fn one(
    state: &AppState,
    attempt: Attempt,
    token: &str,
    retire_blob: bool,
) -> Result<bool, AppError> {
    let database = state.require_database()?;
    Workspace::retire(&state.config.storage_root, &attempt.workspace_id)
        .map_err(|_| AppError::FileStorageUnavailable)?;
    journal::workspace_cleaned_by_id(database, attempt.job_id, attempt.id).await?;
    let current = journal::attempt(database, attempt.id).await?;
    if current.state == "cleaned" {
        release(database, current.id, token).await?;
        return Ok(true);
    }
    if !retire_blob {
        release(database, current.id, token).await?;
        return Ok(false);
    }
    if current.write_started {
        let spec = BlobSpec::new(
            current.size.ok_or(AppError::NotReady)? as u64,
            current.sha256.as_deref().ok_or(AppError::NotReady)?,
        )
        .map_err(|_| AppError::NotReady)?;
        let blob = BlobId::parse(&current.blob_id.to_hex()).map_err(|_| AppError::NotReady)?;
        let store =
            BlobStore::from_profile(database, state.config.clone(), current.storage_profile_id)
                .await
                .map_err(|_| AppError::FileStorageUnavailable)?;
        let cancel = state.export_worker_shutdown.child_token();
        let found = match store.read(blob, spec, cancel.clone()).await {
            Ok(mut stream) => {
                while stream
                    .try_next()
                    .await
                    .map_err(|_| AppError::FileStorageUnavailable)?
                    .is_some()
                {}
                true
            }
            Err(StorageError::NotFound) if current.blob_known => false,
            Err(_) => return Err(AppError::FileStorageUnavailable),
        };
        if found {
            let updated=database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":current.id,"cleanupToken":token,"cleanupExpiresAt":{"$gt":DateTime::now()}},doc!{"$set":{"blobKnown":true,"state":"deleting"}}).await?;
            if updated.matched_count != 1 {
                return Ok(false);
            }
            store
                .delete(blob, cancel)
                .await
                .map_err(|_| AppError::FileStorageUnavailable)?;
        }
    }
    finish(database, &current, token).await
}
async fn claim_cleanup(
    database: &Database,
    id: ObjectId,
) -> Result<Option<(Attempt, String, bool)>, AppError> {
    let token = uuid::Uuid::new_v4().to_string();
    transaction::run(database,(database.clone(),token),|session,(database,token)|async move{
        shared::lock(database,"data-exports",session).await?;let now=DateTime::now();
        let Some(attempt)=database.collection::<Attempt>("data_export_attempts").find_one(doc!{"_id":id,"state":{"$ne":"cleaned"},"nextRunAt":{"$lte":now},"$or":[{"cleanupExpiresAt":null},{"cleanupExpiresAt":{"$lte":now}}]}).session(&mut *session).await? else{return Ok(None)};
        let Some(job)=database.collection::<JobRecord>("data_exports").find_one(doc!{"_id":attempt.job_id}).session(&mut *session).await? else{return Ok(None)};
        let retire_blob=terminal(&job,now)||job.generation!=attempt.generation||job.candidate_id!=Some(attempt.id);
        if !retire_blob&&(attempt.workspace_cleaned||job.lease_expires_at.is_some_and(|expires|expires>now)){return Ok(None);}
        database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":id},doc!{"$set":{"cleanupToken":&*token,"cleanupExpiresAt":DateTime::from_millis(now.timestamp_millis()+60000)}}).session(session).await?;
        Ok(Some((attempt,token.clone(),retire_blob)))
    }.boxed()).await
}
pub async fn run_once(state: &AppState) -> Result<Report, AppError> {
    let Ok(_slot) = state.export_maintenance_slot.clone().try_acquire_owned() else {
        return Ok(Report::default());
    };
    let database = state.require_database()?;
    if !crate::storage::schema::is_initialized(database).await? {
        return Err(AppError::NotReady);
    }
    let now = DateTime::now();
    let attempts=database.collection::<Attempt>("data_export_attempts").find(doc!{"state":{"$ne":"cleaned"},"nextRunAt":{"$lte":now},"$or":[{"cleanupExpiresAt":null},{"cleanupExpiresAt":{"$lte":now}}]}).sort(doc!{"nextRunAt":1,"_id":1}).limit(100).await?.try_collect::<Vec<_>>().await?;
    let mut report = Report::default();
    for attempt in attempts {
        if state.export_worker_shutdown.is_cancelled() || report.scanned >= 10 {
            break;
        }
        let Some(job) = database
            .collection::<JobRecord>("data_exports")
            .find_one(doc! {"_id":attempt.job_id})
            .await?
        else {
            continue;
        };
        let retire_blob = terminal(&job, now)
            || job.generation != attempt.generation
            || job.candidate_id != Some(attempt.id);
        if !retire_blob && attempt.workspace_cleaned {
            continue;
        }
        let Some((attempt, token, retire_blob)) = claim_cleanup(database, attempt.id).await? else {
            continue;
        };
        report.scanned += 1;
        match one(state, attempt.clone(), &token, retire_blob).await {
            Ok(true) => report.retired += 1,
            Ok(false) => report.pending += 1,
            Err(_) => {
                report.pending += 1;
                release(database, attempt.id, &token).await?;
            }
        }
    }
    Ok(report)
}
