use super::{
    cleanup,
    models::{Chunk, Upload, UploadView},
    storage::{BlobId, BlobSpec, BlobStore, StorageError, VerifiedSource},
    uploads,
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::{
        rbac::shared::{self, Actor},
        storage_profiles::service as settings,
    },
    state::AppState,
};
use futures_util::{FutureExt, StreamExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{Bson, DateTime, Document, doc, oid::ObjectId},
};
use std::sync::{Arc, Mutex};
use tokio_util::{io::StreamReader, sync::CancellationToken};

#[derive(Clone)]
struct Claim {
    upload: Upload,
    attempt: ObjectId,
    blob: ObjectId,
    token: String,
    reconcile: bool,
    retry_requested: bool,
}

pub(super) async fn allowed(
    database: &Database,
    owner: ObjectId,
    session: &mut ClientSession,
) -> mongodb::error::Result<bool> {
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {
        "_id":owner,
        "deletedAt":null,
        "status":"active"})
        .projection(doc! {
        "roleIds":1})
        .session(&mut *session)
        .await?;
    let Some(user) = user else { return Ok(false) };
    let roles = user
        .get_array("roleIds")
        .map_err(|_| fail(Failure::NotReady))?;
    Ok(database
        .collection::<Document>("roles")
        .find_one(doc! {
        "_id":{
        "$in":roles}
        ,
        "deletedAt":null,
        "permissions":{
        "$in":["*",
        "files.write"]}
        })
        .session(session)
        .await?
        .is_some())
}

pub async fn enqueue(
    state: &AppState,
    actor: Actor,
    id: ObjectId,
) -> Result<(bool, UploadView), AppError> {
    enqueue_request(state, actor, id, false).await
}

pub async fn retry(
    state: &AppState,
    actor: Actor,
    id: ObjectId,
) -> Result<(bool, UploadView), AppError> {
    enqueue_request(state, actor, id, true).await
}

async fn enqueue_request(
    state: &AppState,
    actor: Actor,
    id: ObjectId,
    explicit_retry: bool,
) -> Result<(bool, UploadView), AppError> {
    let database = state.require_database()?;
    let candidate = ObjectId::new();
    let blob = ObjectId::new();
    let owner = actor.id;
    let accepted = transaction::run(
        database,
        (database.clone(), state.config.clone(), actor),
        |session, (database, config, actor)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "file-storage", session).await?;
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":id,
                    "ownerId":actor.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if row.state == "completed" {
                    return Ok(false);
                }
                if matches!(
                    row.state.as_str(),
                    "queued" | "verifying" | "publishing" | "reconciling" | "unknown"
                ) {
                    if let Some(attempt) = row.publish_attempt_id {
                        let mut update = doc! {"nextRunAt":DateTime::now()};
                        if explicit_retry {
                            update.insert("retryRequested", true);
                        }
                        database
                            .collection::<Document>("file_attempts")
                            .update_one(
                                doc! {
                                "_id":attempt,
                                "state":{
                                "$in":["unknown",
                                "stored", "reconciling"]}
                                },
                                doc! {"$set":update},
                            )
                            .session(session)
                            .await?;
                    }
                    return Ok(true);
                }
                if !matches!(row.state.as_str(), "receiving" | "failed") {
                    return Err(fail(Failure::Conflict("上传当前不能发布")));
                }
                if row.received_count != row.total_chunks || row.received_bytes != row.size {
                    return Err(fail(Failure::Conflict("分片尚未完整接收")));
                }
                if row.attempt_count >= 4096 {
                    return Err(fail(Failure::RateLimited));
                }
                let mut cursor = database
                    .collection::<Chunk>("file_chunks")
                    .find(doc! {
                    "uploadId":id,
                    "state":"accepted"})
                    .sort(doc! {
                    "index":1})
                    .limit(1281)
                    .session(&mut *session)
                    .await?;
                let chunks = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
                if chunks.len() != row.total_chunks as usize
                    || chunks.iter().enumerate().any(|(index, chunk)| {
                        chunk.index != index as i64
                            || chunk.size
                                != (row.size - chunk.index * row.chunk_size).min(row.chunk_size)
                    })
                {
                    return Err(fail(Failure::Conflict("分片清单不完整")));
                }
                if let Some(category) = row.category_id
                    && database
                        .collection::<Document>("file_categories")
                        .find_one(doc! {
                        "_id":category,
                        "deletedAt":null})
                        .session(&mut *session)
                        .await?
                        .is_none()
                {
                    return Err(fail(Failure::Validation("文件分类已不存在")));
                }
                settings::binding(
                    database,
                    Some(row.storage_profile_id),
                    true,
                    config,
                    session,
                )
                .await?;
                let now = DateTime::now();
                database
                    .collection::<Document>("file_attempts")
                    .insert_one(doc! {
                        "_id":candidate,
                    "uploadId":id,
                    "ownerId":actor.id,
                    "storageProfileId":row.storage_profile_id,
                        "kind":"publish",
                    "blobId":blob,
                    "size":row.size,
                    "sha256":&row.sha256,
                    "state":"queued",
                    "retryRequested":false,
                        "leaseToken":null,
                    "leaseUntil":null,
                    "nextRunAt":now,
                    "createdAt":now,
                    "updatedAt":now
                    })
                    .session(&mut *session)
                    .await?;
                database
                    .collection::<Document>("file_uploads")
                    .update_one(
                        doc! {
                        "_id":id},
                        doc! {
                                        "$set":{
                        "state":"queued",
                        "publishAttemptId":candidate,
                        "lastError":null,
                        "updatedAt":now}
                        ,
                                        "$inc":{
                        "version":1i64,
                        "attemptCount":1i64}
                                    },
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "uploads.complete.request", id, session).await?;
                Ok(true)
            }
            .boxed()
        },
    )
    .await?;
    wake(state.clone());
    Ok((accepted, uploads::get(database, owner, id).await?))
}

async fn claim(state: &AppState) -> Result<Option<Claim>, AppError> {
    let database = state.require_database()?;
    let now = DateTime::now();
    if database
        .collection::<Document>("file_attempts")
        .find_one(eligible(now))
        .projection(doc! {
        "_id":1})
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let token = uuid::Uuid::new_v4().to_string();
    transaction::run(
        database,
        (database.clone(), token),
        |session, (database, token)| {
            async move {
                shared::lock(database, "files", session).await?;
                let now = DateTime::now();
                let attempt = database
                    .collection::<Document>("file_attempts")
                    .find_one(eligible(now))
                    .sort(doc! {
                    "createdAt":1,
                    "_id":1})
                    .session(&mut *session)
                    .await?;
                let Some(attempt) = attempt else {
                    return Ok(None);
                };
                let id = attempt
                    .get_object_id("_id")
                    .map_err(|_| fail(Failure::NotReady))?;
                let upload_id = attempt
                    .get_object_id("uploadId")
                    .map_err(|_| fail(Failure::NotReady))?;
                let blob = attempt
                    .get_object_id("blobId")
                    .map_err(|_| fail(Failure::NotReady))?;
                let old = attempt
                    .get_str("state")
                    .map_err(|_| fail(Failure::NotReady))?;
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":upload_id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                let reconcile = !matches!(old, "queued" | "verifying");
                if !reconcile
                    && (row.publish_attempt_id != Some(id)
                        || matches!(row.state.as_str(), "cancelled" | "cancelRequested"))
                {
                    database
                        .collection::<Document>("file_attempts")
                        .update_one(
                            doc! {
                            "_id":id},
                            doc! {
                            "$set":{
                            "state":"cleaned",
                            "updatedAt":now}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                    if row.state == "cancelRequested" {
                        database
                            .collection::<Document>("file_uploads")
                            .update_one(
                                doc! {
                                "_id":upload_id},
                                doc! {
                                "$set":{
                                "state":"cancelled"}
                                },
                            )
                            .session(&mut *session)
                            .await?;
                    }
                    cleanup::schedule(database, &row, session).await?;
                    return Ok(None);
                }
                database.collection::<Document>("file_attempts").update_one(doc!{
"_id":id}
,
doc!{
"$set":{
                "state":if reconcile{
"reconciling"}
else{
"verifying"}
,
"leaseToken":&*token,
                "leaseUntil":DateTime::from_millis(now.timestamp_millis()+60000),
"updatedAt":now
            }
}
).session(&mut *session).await?;
                if row.publish_attempt_id == Some(id)
                    && !matches!(
                        row.state.as_str(),
                        "cancelled" | "cancelRequested" | "completed"
                    )
                {
                    database
                        .collection::<Document>("file_uploads")
                        .update_one(
                            doc! {
                            "_id":upload_id},
                            doc! {
                            "$set":{
                            "state":if reconcile{
                            "reconciling"}
                            else{
                            "verifying"}
                            ,
                            "updatedAt":now}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                }
                Ok(Some(Claim {
                    upload: row,
                    attempt: id,
                    blob,
                    token: token.clone(),
                    reconcile,
                    retry_requested: attempt
                        .get_bool("retryRequested")
                        .map_err(|_| fail(Failure::NotReady))?,
                }))
            }
            .boxed()
        },
    )
    .await
}

fn eligible(now: DateTime) -> Document {
    doc! {
            "kind":"publish",
    "$or":[{
    "state":"queued"}
    ,
    {
    "state":{
    "$in":["verifying",
    "putting",
    "stored",
    "unknown",
    "reconciling",
    "orphanUnknown"]}
    ,
    "leaseUntil":{
    "$lte":now}
    ,
    "nextRunAt":{
    "$lte":now}
    }
    ]
        }
}

pub fn wake(state: AppState) {
    for _ in 0..2 {
        let state = state.clone();
        tokio::spawn(async move {
            let Ok(_permit) = state.file_worker_slots.clone().try_acquire_owned() else {
                return;
            };
            while !state.file_worker_shutdown.is_cancelled() {
                match claim(&state).await {
                    Ok(Some(job)) => {
                        if process(&state, job).await.is_err() {
                            tracing::warn!("file publication deferred");
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        tracing::warn!("file publication claim deferred");
                        break;
                    }
                }
            }
        });
    }
}

pub fn start_service(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
            _=state.file_worker_shutdown.cancelled()=>break,
            _=tokio::time::sleep(std::time::Duration::from_secs(2))=>{
            wake(state.clone());
            maintenance(state.clone());
            }
            }
        }
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            state.file_worker_slots.clone().acquire_many_owned(2),
        )
        .await;
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            state.file_maintenance_slot.clone().acquire_owned(),
        )
        .await;
    })
}

fn maintenance(state: AppState) {
    tokio::spawn(async move {
        let Ok(_permit) = state.file_maintenance_slot.clone().try_acquire_owned() else {
            return;
        };
        if maintenance_once(&state).await.is_err() {
            tracing::warn!("file maintenance deferred");
        }
    });
}
#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceView {
    pub recovered: u64,
    pub retired: u64,
    pub unknown: u64,
    pub staging_removed: u64,
    pub probes_cleaned: u64,
    pub contents_cleaned: u64,
}
pub async fn maintenance_once(state: &AppState) -> Result<MaintenanceView, AppError> {
    let database = state.require_database()?;
    let recovery = super::recovery::background(state).await?;
    let mut result = MaintenanceView {
        recovered: recovery.recovered,
        retired: recovery.retired,
        unknown: recovery.unknown,
        ..Default::default()
    };
    let now = DateTime::now().timestamp_millis();
    if now
        >= state
            .file_next_staging_sweep
            .load(std::sync::atomic::Ordering::Relaxed)
    {
        state
            .file_next_staging_sweep
            .store(now + 60000, std::sync::atomic::Ordering::Relaxed);
        result.staging_removed = super::storage::VerifiedSource::sweep(&state.config)
            .await
            .map_err(|_| AppError::FileStorageUnavailable)?
            .removed;
    }
    result.probes_cleaned = super::probes::background(state).await?;
    let task = database
        .collection::<Document>("file_cleanup_tasks")
        .find_one(doc! {
        "state":"pending",
        "nextRunAt":{
        "$lte":DateTime::now()}
        })
        .sort(doc! {
        "nextRunAt":1,
        "updatedAt":1,
        "_id":1})
        .await;
    if let Ok(Some(task)) = task
        && let Ok(id) = task.get_object_id("uploadId")
        && let Ok(Some(parent)) = database
            .collection::<Upload>("file_uploads")
            .find_one(doc! {
            "_id":id})
            .await
    {
        result.contents_cleaned = cleanup::run(state, parent.owner_id, id).await?.processed;
    }
    Ok(result)
}

pub fn stop_service(state: &AppState) {
    state.file_worker_shutdown.cancel();
}
pub(crate) async fn recheck(
    state: &AppState,
    owner: ObjectId,
    id: ObjectId,
) -> Result<(), AppError> {
    let database = state.require_database()?;
    uploads::get_record(database, owner, id).await?;
    database
        .collection::<Document>("file_attempts")
        .update_many(
            doc! {
            "uploadId":id,
            "kind":"publish",
            "state":{
            "$in":["stored",
            "unknown",
            "orphanUnknown"]}
            },
            doc! {
            "$set":{
            "nextRunAt":DateTime::now()}
            },
        )
        .await?;
    wake(state.clone());
    Ok(())
}

async fn process(state: &AppState, job: Claim) -> Result<(), AppError> {
    let cancel = CancellationToken::new();
    let done = CancellationToken::new();
    let watcher_state = state.clone();
    let watched = job.clone();
    let watched_cancel = cancel.clone();
    let watched_done = done.clone();
    let watcher = tokio::spawn(async move {
        let mut tick = 0;
        loop {
            tokio::select! {
            _=watched_done.cancelled()=>break,
            _=watcher_state.file_worker_shutdown.cancelled()=>{
            watched_cancel.cancel();
            break}
            ,
            _=tokio::time::sleep(std::time::Duration::from_secs(1))=>{
            }
            }
            let Ok(database) = watcher_state.require_database() else {
                watched_cancel.cancel();
                break;
            };
            let row = database
                .collection::<Upload>("file_uploads")
                .find_one(doc! {
                "_id":watched.upload.id})
                .await;
            if let Ok(Some(row)) = row
                && !watched.reconcile
                && matches!(row.state.as_str(), "cancelRequested" | "cancelled")
            {
                watched_cancel.cancel();
            }
            tick += 1;
            if tick % 10 == 0 {
                let result=database.collection::<Document>("file_attempts").update_one(doc!{
"_id":watched.attempt,
"leaseToken":&watched.token,
"state":{
"$in":["verifying",
"putting",
"reconciling",
"stored"]}
}
,
doc!{
"$set":{
"leaseUntil":DateTime::from_millis(DateTime::now().timestamp_millis()+60000)}
}
).await;
                if !matches!(result,
Ok(value)if value.matched_count==1)
                {
                    watched_cancel.cancel();
                    break;
                }
            }
        }
    });
    let result = execute(state, &job, cancel).await;
    done.cancel();
    let _ = watcher.await;
    if result.is_ok() {
        let _ = cleanup::run(state, job.upload.owner_id, job.upload.id).await;
    }
    result
}

async fn execute(state: &AppState, job: &Claim, cancel: CancellationToken) -> Result<(), AppError> {
    let database = state.require_database()?;
    let store = match BlobStore::from_profile(
        database,
        state.config.clone(),
        job.upload.storage_profile_id,
    )
    .await
    {
        Ok(store) => store,
        Err(_) => {
            return if job.reconcile {
                unknown(state, job, "STORAGE_UNAVAILABLE").await
            } else {
                no_write(state, job, "STORAGE_UNAVAILABLE", None).await
            };
        }
    };
    let spec = BlobSpec::new(job.upload.size as u64, &job.upload.sha256)
        .map_err(|_| AppError::NotReady)?;
    let blob = BlobId::parse(&job.blob.to_hex()).map_err(|_| AppError::NotReady)?;
    if job.reconcile {
        match store.read(blob, spec, cancel).await {
            Ok(mut stream) => {
                if retire_observed(state, job).await? {
                    return Ok(());
                }
                while let Some(chunk) = stream.next().await {
                    if let Err(error) = chunk {
                        if error == StorageError::IntegrityMismatch
                            && job.retry_requested
                            && supersede(state, job, true).await?
                        {
                            return Ok(());
                        }
                        return unknown(state, job, "STORAGE_CONTENT_UNAVAILABLE").await;
                    }
                }
                return commit_or_defer(state, job).await;
            }
            Err(StorageError::NotFound) if job.retry_requested => {
                if supersede(state, job, false).await? {
                    return Ok(());
                }
                return unknown(state, job, "STORAGE_OUTCOME_UNKNOWN").await;
            }
            Err(StorageError::IntegrityMismatch) => {
                if retire_observed(state, job).await?
                    || job.retry_requested && supersede(state, job, true).await?
                {
                    return Ok(());
                }
                return unknown(state, job, "STORAGE_CONTENT_UNAVAILABLE").await;
            }
            Err(_) => return unknown(state, job, "STORAGE_OUTCOME_UNKNOWN").await,
        }
    }
    let rows = database
        .collection::<Chunk>("file_chunks")
        .find(doc! {
        "uploadId":job.upload.id,
        "state":"accepted"})
        .sort(doc! {
        "index":1})
        .limit(1281)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let chunk_store = Arc::new(
        BlobStore::local(state.config.clone(), ".tri-upload-chunks")
            .await
            .map_err(|_| AppError::FileStorageUnavailable)?,
    );
    let invalid = Arc::new(Mutex::new(None::<(i64, ObjectId, &'static str)>));
    let error_slot = invalid.clone();
    let read_cancel = cancel.clone();
    let raw = futures_util::stream::iter(rows)
        .then(move |chunk| {
            let store = chunk_store.clone();
            let invalid = error_slot.clone();
            let cancel = read_cancel.clone();
            async move {
                let index = chunk.index;
                let blob = chunk.blob_id;
                let record_error = move |error: StorageError| {
                    let code = match error {
                        StorageError::NotFound => "CHUNK_MISSING",
                        StorageError::IntegrityMismatch => "CHUNK_CORRUPT",
                        _ => "CHUNK_UNAVAILABLE",
                    };
                    if let Ok(mut slot) = invalid.lock() {
                        *slot = Some((index, blob, code));
                    }
                    std::io::Error::other("chunk unavailable")
                };
                let stream = store
                    .read(
                        BlobId::parse(&blob.to_hex()).map_err(record_error.clone())?,
                        BlobSpec::new(chunk.size as u64, &chunk.sha256)
                            .map_err(record_error.clone())?,
                        cancel,
                    )
                    .await
                    .map_err(record_error.clone())?;
                let mapped: std::pin::Pin<
                    Box<
                        dyn futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>>
                            + Send,
                    >,
                > = Box::pin(stream.map(move |value| value.map_err(record_error.clone())));
                Ok::<_, std::io::Error>(mapped)
            }
            .boxed()
        })
        .try_flatten();
    let raw: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send>,
    > = Box::pin(raw);
    let source =
        VerifiedSource::stage(&state.config, StreamReader::new(raw), spec, cancel.clone()).await;
    let source = match source {
        Ok(source) => source,
        Err(error) => {
            let invalid = *invalid.lock().map_err(|_| AppError::Internal)?;
            let code = if cancel.is_cancelled() {
                "CANCELLED"
            } else {
                invalid.as_ref().map(|item| item.2).unwrap_or(
                    if error == StorageError::IntegrityMismatch {
                        "CHECKSUM_MISMATCH"
                    } else if error == StorageError::Cancelled {
                        "CANCELLED"
                    } else {
                        "STORAGE_UNAVAILABLE"
                    },
                )
            };
            return no_write(state, job, code, invalid).await;
        }
    };
    if !mark_putting(state, job).await? {
        return Ok(());
    }
    match store.put(blob, source, cancel).await {
        Ok(_) => commit_or_defer(state, job).await,
        Err(StorageError::OutcomeUnknown) => unknown(state, job, "STORAGE_OUTCOME_UNKNOWN").await,
        Err(error) => {
            no_write(
                state,
                job,
                if error == StorageError::Cancelled {
                    "CANCELLED"
                } else {
                    "STORAGE_UNAVAILABLE"
                },
                None,
            )
            .await
        }
    }
}

async fn retire_observed(state: &AppState, job: &Claim) -> Result<bool, AppError> {
    let database = state.require_database()?;
    transaction::run(
        database,
        (database.clone(), job.clone()),
        |session, (database, job)| {
            async move {
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {"_id":job.upload.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                if row.publish_attempt_id == Some(job.attempt)
                    && !matches!(
                        row.state.as_str(),
                        "cancelled" | "cancelRequested" | "completed"
                    )
                {
                    return Ok(false);
                }
                let changed = database
                    .collection::<Document>("file_attempts")
                    .update_one(
                        doc! {"_id":job.attempt,"leaseToken":&job.token,"state":"reconciling"},
                        doc! {"$set":{"state":"orphan","updatedAt":DateTime::now()}},
                    )
                    .session(&mut *session)
                    .await?;
                if changed.matched_count == 0 {
                    return Ok(false);
                }
                if row.publish_attempt_id == Some(job.attempt) && row.state == "cancelRequested" {
                    database
                        .collection::<Document>("file_uploads")
                        .update_one(doc! {"_id":row.id}, doc! {"$set":{"state":"cancelled"}})
                        .session(&mut *session)
                        .await?;
                }
                cleanup::schedule(database, &row, session).await?;
                Ok(true)
            }
            .boxed()
        },
    )
    .await
}

async fn supersede(state: &AppState, job: &Claim, observed: bool) -> Result<bool, AppError> {
    let database = state.require_database()?;
    let candidate = ObjectId::new();
    let blob = ObjectId::new();
    transaction::run(database, (database.clone(), job.clone()), |session, (database, job)| {
        async move {
            shared::lock(database, "rbac", session).await?;
            shared::lock(database, "files", session).await?;
            let now = DateTime::now();
            let owned = database.collection::<Document>("file_attempts")
                .find_one(doc! {"_id":job.attempt,"leaseToken":&job.token,"state":"reconciling","retryRequested":true,"leaseUntil":{"$gt":now}})
                .session(&mut *session).await?;
            if owned.is_none() { return Ok(false); }
            let row = database.collection::<Upload>("file_uploads")
                .find_one(doc! {"_id":job.upload.id}).session(&mut *session).await?
                .ok_or_else(|| fail(Failure::NotReady))?;
            if row.publish_attempt_id != Some(job.attempt)
                || matches!(row.state.as_str(), "cancelled" | "cancelRequested" | "completed")
                || !allowed(database, row.owner_id, session).await? {
                return Ok(false);
            }
            if row.attempt_count >= 4096 { return Err(fail(Failure::RateLimited)); }
            database.collection::<Document>("file_attempts").update_one(
                doc! {"_id":job.attempt,"leaseToken":&job.token},
                doc! {"$set":{"state":if observed {"orphan"} else {"orphanUnknown"},"retryRequested":false,"leaseToken":null,"leaseUntil":now,"nextRunAt":DateTime::from_millis(now.timestamp_millis()+30000),"updatedAt":now}}
            ).session(&mut *session).await?;
            database.collection::<Document>("file_attempts").insert_one(doc! {
                "_id":candidate,"uploadId":row.id,"ownerId":row.owner_id,
                "storageProfileId":row.storage_profile_id,"kind":"publish","blobId":blob,
                "size":row.size,"sha256":&row.sha256,"state":"queued","retryRequested":false,
                "leaseToken":null,"leaseUntil":null,"nextRunAt":now,"createdAt":now,"updatedAt":now
            }).session(&mut *session).await?;
            database.collection::<Document>("file_uploads").update_one(
                doc! {"_id":row.id,"publishAttemptId":job.attempt},
                doc! {"$set":{"state":"queued","publishAttemptId":candidate,"lastError":null,"updatedAt":now},"$inc":{"version":1i64,"attemptCount":1i64}}
            ).session(&mut *session).await?;
            cleanup::schedule(database, &row, session).await?;
            database.collection::<Document>("audit_events").insert_one(
                crate::common::observability::event(doc! {"category":"administration","action":"uploads.retry","actorId":row.owner_id,"targetId":row.id,"outcome":"success","createdAt":now})
            ).session(session).await?;
            Ok(true)
        }.boxed()
    }).await
}

async fn mark_putting(state: &AppState, job: &Claim) -> Result<bool, AppError> {
    let database = state.require_database()?;
    transaction::run(
        database,
        (database.clone(), job.clone()),
        |session, (database, job)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":job.upload.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                if database
                    .collection::<Document>("file_attempts")
                    .find_one(doc! {
                    "_id":job.attempt,
                    "leaseToken":&job.token,
                    "state":"verifying"})
                    .session(&mut *session)
                    .await?
                    .is_none()
                {
                    return Ok(false);
                }
                if row.publish_attempt_id != Some(job.attempt) {
                    return Ok(false);
                }
                if matches!(row.state.as_str(), "cancelRequested" | "cancelled")
                    || !allowed(database, row.owner_id, session).await?
                {
                    let cancelled = matches!(row.state.as_str(), "cancelRequested" | "cancelled");
                    let now = DateTime::now();
                    database
                        .collection::<Document>("file_attempts")
                        .update_one(
                            doc! {
                            "_id":job.attempt,
                            "leaseToken":&job.token},
                            doc! {
                            "$set":{
                            "state":"cleaned",
                            "updatedAt":now}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                    database
                        .collection::<Document>("file_uploads")
                        .update_one(
                            doc! {
                            "_id":row.id},
                            doc! {
                            "$set":{
                            "state":if cancelled{
                            "cancelled"}
                            else{
                            "failed"}
                            ,
                            "lastError":if cancelled{
                            Bson::Null}
                            else{
                            Bson::String("PERMISSION_REVOKED".into())}
                            ,
                            "updatedAt":now}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                    if cancelled {
                        cleanup::schedule(database, &row, session).await?;
                    }
                    return Ok(false);
                }
                let result = database
                    .collection::<Document>("file_attempts")
                    .update_one(
                        doc! {
                        "_id":job.attempt,
                        "leaseToken":&job.token,
                        "state":"verifying"},
                        doc! {
                        "$set":{
                        "state":"putting",
                        "updatedAt":DateTime::now()}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                if result.matched_count == 0 {
                    return Ok(false);
                }
                database
                    .collection::<Document>("file_uploads")
                    .update_one(
                        doc! {
                        "_id":row.id},
                        doc! {
                        "$set":{
                        "state":"publishing"}
                        },
                    )
                    .session(session)
                    .await?;
                Ok(true)
            }
            .boxed()
        },
    )
    .await
}

async fn no_write(
    state: &AppState,
    job: &Claim,
    code: &'static str,
    invalid: Option<(i64, ObjectId, &'static str)>,
) -> Result<(), AppError> {
    let database = state.require_database()?;
    transaction::run(
        database,
        (database.clone(), job.clone()),
        |session, (database, job)| {
            async move {
                shared::lock(database, "files", session).await?;
                let owned = database
                    .collection::<Document>("file_attempts")
                    .find_one(doc! {
                    "_id":job.attempt,
                    "leaseToken":&job.token})
                    .session(&mut *session)
                    .await?;
                if owned.is_none() {
                    return Ok(());
                }
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":job.upload.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                let cancelled = matches!(row.state.as_str(), "cancelRequested" | "cancelled");
                let restarting = code == "CANCELLED" && !cancelled;
                let repair =
                    invalid.filter(|value| matches!(value.2, "CHUNK_MISSING" | "CHUNK_CORRUPT"));
                let now = DateTime::now();
                database
                    .collection::<Document>("file_attempts")
                    .update_one(
                        doc! {
                        "_id":job.attempt,
                        "leaseToken":&job.token},
                        doc! {
                        "$set":{
                        "state":if restarting{
                        "queued"}
                        else{
                        "cleaned"}
                        ,
                        "leaseToken":null,
                        "leaseUntil":null,
                        "nextRunAt":now,
                        "updatedAt":now}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                if row.publish_attempt_id != Some(job.attempt) {
                    return Ok(());
                }
                if let Some((index, blob, _)) = repair
                    && let Some(chunk) = database
                        .collection::<Chunk>("file_chunks")
                        .find_one(doc! {
                        "uploadId":row.id,
                        "index":index,
                        "blobId":blob,
                        "state":"accepted"})
                        .session(&mut *session)
                        .await?
                {
                    database
                        .collection::<Document>("file_chunks")
                        .update_one(
                            doc! {
                            "uploadId":row.id,
                            "index":index},
                            doc! {
                            "$set":{
                            "state":"invalid"}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                    database
                        .collection::<Document>("file_attempts")
                        .update_one(
                            doc! {
                            "uploadId":row.id,
                            "blobId":blob,
                            "kind":"chunk"},
                            doc! {
                            "$set":{
                            "state":"orphan",
                            "updatedAt":now}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                    database
                        .collection::<Document>("file_uploads")
                        .update_one(
                            doc! {
                            "_id":row.id},
                            doc! {
                            "$inc":{
                            "receivedCount":-1i64,
                            "receivedBytes":-chunk.size}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                    cleanup::schedule(database, &row, session).await?;
                }
                database
                    .collection::<Document>("file_uploads")
                    .update_one(
                        doc! {
                        "_id":row.id},
                        doc! {
                        "$set":{
                                    "state":if cancelled{
                        "cancelled"}
                        else if restarting{
                        "queued"}
                        else if repair.is_some(){
                        "receiving"}
                        else{
                        "failed"}
                        ,
                                    "lastError":if cancelled||restarting{
                        Bson::Null}
                        else{
                        Bson::String(code.into())}
                        ,
                        "updatedAt":now
                                }
                        ,
                        "$inc":{
                        "version":1i64}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                if cancelled {
                    cleanup::schedule(database, &row, session).await?;
                }
                Ok(())
            }
            .boxed()
        },
    )
    .await
}

async fn unknown(state: &AppState, job: &Claim, code: &'static str) -> Result<(), AppError> {
    let database = state.require_database()?;
    transaction::run(
        database,
        (database.clone(), job.clone()),
        |session, (database, job)| {
            async move {
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":job.upload.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                let cancelled = matches!(row.state.as_str(), "cancelRequested" | "cancelled")
                    || row.publish_attempt_id != Some(job.attempt)
                    || row.state == "completed";
                let now = DateTime::now();
                let changed = database
                    .collection::<Document>("file_attempts")
                    .update_one(
                        doc! {
                        "_id":job.attempt,
                        "leaseToken":&job.token},
                        doc! {
                        "$set":{
                                    "state":if cancelled{
                        "orphanUnknown"}
                        else{
                        "unknown"}
                        ,
                        "leaseUntil":now,
                        "nextRunAt":DateTime::from_millis(now.timestamp_millis()+30000),
                        "updatedAt":now
                                }
                        },
                    )
                    .session(&mut *session)
                    .await?;
                if changed.matched_count == 0 {
                    return Ok(());
                }
                if row.publish_attempt_id == Some(job.attempt) && row.state != "completed" {
                    database
                        .collection::<Document>("file_uploads")
                        .update_one(
                            doc! {
                            "_id":row.id},
                            doc! {
                            "$set":{
                            "state":if cancelled{
                            "cancelled"}
                            else{
                            "unknown"}
                            ,
                            "lastError":code,
                            "updatedAt":now}
                            ,
                            "$inc":{
                            "version":1i64}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                }
                if cancelled {
                    cleanup::schedule(database, &row, session).await?;
                }
                Ok(())
            }
            .boxed()
        },
    )
    .await
}

async fn commit_or_defer(state: &AppState, job: &Claim) -> Result<(), AppError> {
    if let Err(error) = publish(state, job).await {
        let database = state.require_database()?;
        let _ = transaction::run(
            database,
            (database.clone(), job.clone()),
            |session, (database, job)| {
                async move {
                    shared::lock(database, "files", session).await?;
                    let now = DateTime::now();
                    let changed = database
                        .collection::<Document>("file_attempts")
                        .update_one(
                            doc! {
                            "_id":job.attempt,
                            "leaseToken":&job.token,
                            "state":{
                            "$in":["putting",
                            "stored",
                            "reconciling"]}
                            },
                            doc! {
                            "$set":{
                            "state":"stored",
                            "leaseUntil":now,
                            "nextRunAt":DateTime::from_millis(now.timestamp_millis()+5000),
                            "updatedAt":now}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                    if changed.matched_count == 1 {
                        database
                            .collection::<Document>("file_uploads")
                            .update_one(
                                doc! {
                                "_id":job.upload.id,
                                "publishAttemptId":job.attempt,
                                "state":{
                                "$nin":["completed",
                                "cancelled",
                                "cancelRequested"]}
                                },
                                doc! {
                                "$set":{
                                "state":"unknown",
                                "lastError":"PERSISTENCE_PENDING",
                                "updatedAt":now}
                                },
                            )
                            .session(session)
                            .await?;
                    }
                    Ok(())
                }
                .boxed()
            },
        )
        .await;
        return Err(error);
    }
    Ok(())
}

async fn publish(state: &AppState, job: &Claim) -> Result<(), AppError> {
    let database = state.require_database()?;
    transaction::run(
        database,
        (database.clone(), job.clone()),
        |session, (database, job)| {
            async move {
                shared::lock(database, "rbac", session).await?;
                shared::lock(database, "files", session).await?;
                let owned = database
                    .collection::<Document>("file_attempts")
                    .find_one(doc! {
                    "_id":job.attempt,
                    "leaseToken":&job.token,
                    "state":{
                    "$in":["putting",
                    "stored",
                    "reconciling"]}
                    })
                    .session(&mut *session)
                    .await?;
                if owned.is_none() {
                    return Ok(());
                }
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":job.upload.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                let cancelled = matches!(row.state.as_str(), "cancelRequested" | "cancelled")
                    || row.publish_attempt_id != Some(job.attempt)
                    || row.state == "completed";
                let permitted = allowed(database, row.owner_id, session).await?;
                let now = DateTime::now();
                if cancelled || !permitted {
                    database
                        .collection::<Document>("file_attempts")
                        .update_one(
                            doc! {
                            "_id":job.attempt},
                            doc! {
                            "$set":{
                            "state":"orphan",
                            "updatedAt":now}
                            },
                        )
                        .session(&mut *session)
                        .await?;
                    if row.publish_attempt_id == Some(job.attempt) && row.state != "completed" {
                        database
                            .collection::<Document>("file_uploads")
                            .update_one(
                                doc! {
                                "_id":row.id},
                                doc! {
                                "$set":{
                                "state":if cancelled{
                                "cancelled"}
                                else{
                                "failed"}
                                ,
                                "lastError":if cancelled{
                                Bson::Null}
                                else{
                                Bson::String("PERMISSION_REVOKED".into())}
                                }
                                },
                            )
                            .session(&mut *session)
                            .await?;
                    }
                    cleanup::schedule(database, &row, session).await?;
                    return Ok(());
                }
                database
                    .collection::<Document>("files")
                    .insert_one(doc! {
                        "_id":row.id,
                    "uploadId":row.id,
                    "ownerId":row.owner_id,
                    "name":&row.name,
                    "mediaType":&row.media_type,
                        "size":row.size,
                    "sha256":&row.sha256,
                    "categoryId":row.category_id,
                    "storageProfileId":row.storage_profile_id,
                        "blobId":job.blob,
                    "publishAttemptId":job.attempt,
                    "version":1i64,
                    "createdAt":now,
                    "updatedAt":now,
                    "deletedAt":null
                    })
                    .session(&mut *session)
                    .await?;
                database
                    .collection::<Document>("file_attempts")
                    .update_one(
                        doc! {
                        "_id":job.attempt},
                        doc! {
                        "$set":{
                        "state":"published",
                        "updatedAt":now,
                        "leaseUntil":now}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                database
                    .collection::<Document>("file_uploads")
                    .update_one(
                        doc! {
                        "_id":row.id},
                        doc! {
                        "$set":{
                        "state":"completed",
                        "fileId":row.id,
                        "completedAt":now,
                        "lastError":null,
                        "updatedAt":now}
                        ,
                        "$inc":{
                        "version":1i64}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                cleanup::schedule(database, &row, session).await?;
                database
                    .collection::<Document>("audit_events")
                    .insert_one(crate::common::observability::event(doc! {
                    "category":"administration",
                    "action":"files.publish",
                    "actorId":row.owner_id,
                    "targetId":row.id,
                    "outcome":"success",
                    "createdAt":now}))
                    .session(session)
                    .await?;
                Ok(())
            }
            .boxed()
        },
    )
    .await
}
