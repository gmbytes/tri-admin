use super::super::export_writer::{ExportArtifact, ExportError, ExportLimits, ExportWriter};
use super::{
    authorization,
    journal::{self, Claim},
    query::{self, Message},
    workspace::{Reservation, Workspace, WorkspaceError},
};
use crate::{
    common::error::AppError,
    modules::{
        files::storage::{BlobId, BlobSpec, BlobStore, VerifiedSource},
        rbac::shared::Actor,
    },
    state::AppState,
};
use futures_util::TryStreamExt;
use mongodb::bson::{DateTime, Document, doc};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn code(error: &AppError) -> &'static str {
    match error {
        AppError::Forbidden => "PERMISSION_REVOKED",
        AppError::Validation(value) if value == "ROW_LIMIT" => "ROW_LIMIT",
        AppError::Conflict(value) if value == "LEASE_LOST" => "LEASE_LOST",
        AppError::Conflict(_) => "SCOPE_CHANGED",
        AppError::FileStorageUnavailable => "FILE_STORAGE_UNAVAILABLE",
        _ => "EXPORT_FAILED",
    }
}
fn writer_error(error: ExportError) -> AppError {
    match error {
        ExportError::RowLimit => AppError::Validation("ROW_LIMIT".into()),
        ExportError::Cancelled => AppError::Conflict("LEASE_LOST".into()),
        _ => AppError::Validation("导出编码或磁盘校验失败".into()),
    }
}
async fn generate(
    state: &AppState,
    claim: &Claim,
    actor: &Actor,
    workspace: Workspace,
    cancel: CancellationToken,
) -> Result<(Workspace, Result<ExportArtifact, AppError>), AppError> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Message>(4);
    let job = claim.job.clone();
    let owner = claim.clone();
    let database = state.require_database()?.clone();
    let handle = tokio::runtime::Handle::current();
    let writer_cancel = cancel.clone();
    let consumer = tokio::task::spawn_blocking(move || {
        let result = (|| {
            let file = workspace
                .new_output()
                .map_err(|_| AppError::FileStorageUnavailable)?;
            let limits = ExportLimits {
                max_rows: job.meta.max_rows,
                ..ExportLimits::default()
            };
            let mut writer = ExportWriter::new(
                job.meta.format,
                job.spec
                    .columns
                    .iter()
                    .map(|column| column.header.clone())
                    .collect(),
                file,
                workspace.path(),
                limits,
                writer_cancel,
            )
            .map_err(writer_error)?;
            let mut ids = Vec::with_capacity(1000);
            let mut batch = 0u32;
            let mut count = 0u32;
            while let Some(message) = receiver.blocking_recv() {
                match message {
                    Message::Row(row) => {
                        writer.write_row(&row.cells).map_err(writer_error)?;
                        count += 1;
                        if job.meta.resource == "users" {
                            ids.push(row.id);
                        }
                        if count.is_multiple_of(1000) {
                            if job.meta.resource == "users" {
                                handle.block_on(authorization::record_manifest_batch(
                                    &database,
                                    job.meta.id,
                                    job.generation,
                                    &owner.token,
                                    batch,
                                    std::mem::take(&mut ids),
                                ))?;
                                batch += 1;
                            } else {
                                handle.block_on(journal::progress(&database, &owner, count))?;
                            }
                        }
                    }
                    Message::Finish => {
                        if job.meta.resource == "users" && !ids.is_empty() {
                            handle.block_on(authorization::record_manifest_batch(
                                &database,
                                job.meta.id,
                                job.generation,
                                &owner.token,
                                batch,
                                ids,
                            ))?;
                        } else if job.meta.resource != "users" {
                            handle.block_on(journal::progress(&database, &owner, count))?;
                        }
                        return writer.finish().map_err(writer_error);
                    }
                }
            }
            Err(AppError::Conflict("LEASE_LOST".into()))
        })();
        (workspace, result)
    });
    let produced = query::stream(
        state.require_database()?,
        &claim.job,
        actor,
        sender.clone(),
        cancel,
    )
    .await;
    if produced.is_ok() {
        let _ = sender.send(Message::Finish).await;
    }
    drop(sender);
    let (workspace, result) = consumer.await.map_err(|_| AppError::Internal)?;
    Ok((
        workspace,
        match (produced, result) {
            (Err(error), _) if matches!(error, AppError::Validation(_) | AppError::Database(_)) => {
                Err(error)
            }
            (_, result) => result,
        },
    ))
}
async fn publish(state: &AppState, claim: &Claim) -> Result<(), AppError> {
    let database = state.require_database()?;
    let row = journal::current(database, claim).await?;
    let actor = authorization::execution_actor(database, row.actor_id, &row.meta.resource).await?;
    authorization::verify_user_manifest(database, &row, &actor).await?;
    let attempt = journal::attempt(database, claim.attempt.id).await?;
    journal::complete(database, claim, actor, &attempt).await
}
async fn execute(
    state: &AppState,
    claim: &Claim,
    workspace: &mut Option<Workspace>,
    cancel: CancellationToken,
) -> Result<(), AppError> {
    let database = state.require_database()?;
    if claim.fresh {
        let actor =
            authorization::execution_actor(database, claim.job.actor_id, &claim.job.meta.resource)
                .await?;
        let (owned, result) = generate(
            state,
            claim,
            &actor,
            workspace.take().ok_or(AppError::NotReady)?,
            cancel.clone(),
        )
        .await?;
        *workspace = Some(owned);
        let artifact = result?;
        journal::prepared(
            database,
            claim,
            artifact.spec.size(),
            artifact.spec.sha256(),
            artifact.row_count,
        )
        .await?;
        drop(artifact);
    }
    let attempt = journal::attempt(database, claim.attempt.id).await?;
    if !attempt.write_started && (attempt.state == "generating" || workspace.is_none()) {
        journal::requeue(database, claim).await?;
        return Ok(());
    }
    let actor =
        authorization::execution_actor(database, claim.job.actor_id, &claim.job.meta.resource)
            .await?;
    let current = journal::current(database, claim).await?;
    authorization::verify_user_manifest(database, &current, &actor).await?;
    let spec = BlobSpec::new(
        attempt.size.ok_or(AppError::NotReady)? as u64,
        attempt.sha256.as_deref().ok_or(AppError::NotReady)?,
    )
    .map_err(|_| AppError::NotReady)?;
    let blob = BlobId::parse(&attempt.blob_id.to_hex()).map_err(|_| AppError::NotReady)?;
    let store = BlobStore::from_profile(database, state.config.clone(), attempt.storage_profile_id)
        .await
        .map_err(|_| AppError::FileStorageUnavailable)?;
    if attempt.write_started {
        let observed = match store.read(blob, spec.clone(), cancel.clone()).await {
            Ok(mut stream) => {
                let mut valid = true;
                loop {
                    match stream.try_next().await {
                        Ok(Some(_)) => (),
                        Ok(None) => break,
                        Err(_) => {
                            valid = false;
                            break;
                        }
                    }
                }
                valid
            }
            Err(_) => false,
        };
        if !observed {
            journal::unknown(database, claim, "PUBLICATION_PENDING").await?;
            return Ok(());
        }
        database
            .collection::<Document>("data_export_attempts")
            .update_one(
                doc! {"_id":attempt.id},
                doc! {"$set":{"blobKnown":true,"state":"stored"}},
            )
            .await?;
    } else {
        let file = workspace
            .as_ref()
            .ok_or(AppError::NotReady)?
            .output()
            .map_err(|_| AppError::FileStorageUnavailable)?;
        let source = VerifiedSource::stage(
            &state.config,
            tokio::fs::File::from_std(file),
            spec,
            cancel.clone(),
        )
        .await
        .map_err(|_| AppError::FileStorageUnavailable)?;
        journal::putting(database, claim).await?;
        if store.put(blob, source, cancel).await.is_err() {
            journal::unknown(database, claim, "PUBLICATION_PENDING").await?;
            return Ok(());
        }
        database
            .collection::<Document>("data_export_attempts")
            .update_one(
                doc! {"_id":attempt.id},
                doc! {"$set":{"state":"stored","blobKnown":true,"updatedAt":DateTime::now()}},
            )
            .await?;
    }
    match publish(state, claim).await {
        Ok(()) => Ok(()),
        Err(AppError::Database(_) | AppError::Internal) => {
            journal::unknown(database, claim, "PUBLICATION_PENDING").await?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}
pub async fn run_once(state: &AppState) -> Result<bool, AppError> {
    if state.export_worker_shutdown.is_cancelled() {
        return Ok(false);
    }
    let Ok(_slot) = state.export_worker_slots.clone().try_acquire_owned() else {
        return Ok(false);
    };
    let database = state.require_database()?;
    if !crate::storage::schema::is_initialized(database).await? {
        return Ok(false);
    }
    let Some(next) = journal::next(database).await? else {
        return Ok(false);
    };
    let reservation = if next.meta.state == super::models::JobState::Queued {
        match Reservation::reserve(&state.config.storage_root) {
            Ok(value) => Some(value),
            Err(_) => {
                database.collection::<Document>("data_exports").update_one(doc!{"_id":next.meta.id,"state":"queued","nextRunAt":{"$lte":DateTime::now()}},doc!{"$set":{"lastError":"WORKSPACE_UNAVAILABLE","nextRunAt":DateTime::from_millis(DateTime::now().timestamp_millis()+60000),"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).await?;
                return Err(AppError::FileStorageUnavailable);
            }
        }
    } else {
        None
    };
    let Some(claim) = journal::claim(
        state,
        next.meta.id,
        reservation.as_ref().map(|value| value.id().to_owned()),
    )
    .await?
    else {
        return Ok(false);
    };
    let mut workspace = if let Some(reserved) = reservation {
        Some(
            reserved
                .activate()
                .map_err(|_| AppError::FileStorageUnavailable)?,
        )
    } else {
        match Workspace::open(&state.config.storage_root, &claim.attempt.workspace_id) {
            Ok(workspace) => Some(workspace),
            Err(WorkspaceError::Missing) => None,
            Err(WorkspaceError::Busy) => {
                journal::unknown(database, &claim, "WORKSPACE_BUSY").await?;
                return Ok(true);
            }
            Err(_) => {
                journal::failed(database, &claim, "WORKSPACE_UNCONFIRMED").await?;
                return Ok(true);
            }
        }
    };
    let cancel = state.export_worker_shutdown.child_token();
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    interval.tick().await;
    let result = {
        let work = tokio::time::timeout(
            Duration::from_secs(1800),
            execute(state, &claim, &mut workspace, cancel.clone()),
        );
        tokio::pin!(work);
        loop {
            tokio::select! {result=&mut work=>break result.unwrap_or_else(|_|Err(AppError::Validation("导出执行超时".into()))),_=state.export_worker_shutdown.cancelled()=>{cancel.cancel();break work.await.unwrap_or_else(|_|Err(AppError::Internal));},_=interval.tick()=>{if !journal::renew(database,&claim).await?{cancel.cancel();}}}
        }
    };
    if let Err(error) = result {
        let attempt = journal::attempt(database, claim.attempt.id).await?;
        if state.export_worker_shutdown.is_cancelled() && !attempt.write_started {
            journal::requeue(database, &claim).await?;
        } else if attempt.write_started
            && !matches!(error, AppError::Forbidden | AppError::Conflict(_))
        {
            journal::unknown(database, &claim, "PUBLICATION_PENDING").await?;
        } else {
            journal::failed(database, &claim, code(&error)).await?;
        }
    }
    if let Some(owned) = workspace {
        if owned.clean().is_ok() {
            journal::workspace_cleaned(database, &claim).await?;
        }
    } else if journal::attempt(database, claim.attempt.id)
        .await?
        .workspace_cleaned
    {
        journal::workspace_cleaned(database, &claim).await?;
    }
    Ok(true)
}
pub fn start_service(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let state = state.clone();
            tasks.spawn(async move{
            while !state.export_worker_shutdown.is_cancelled() {
                if let Err(error)=run_once(&state).await && !matches!(error,AppError::NotReady){tracing::warn!(code="EXPORT_WORKER_FAILURE","export worker iteration deferred");}
                tokio::select!{_=state.export_worker_shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(1))=>{}}
            }
        });
        }
        let maintenance_state = state.clone();
        tasks.spawn(async move{
            while !maintenance_state.export_worker_shutdown.is_cancelled(){
                if let Err(error)=super::maintenance::run_once(&maintenance_state).await && !matches!(error,AppError::NotReady){tracing::warn!(code="EXPORT_MAINTENANCE_FAILURE","export maintenance deferred");}
                tokio::select!{_=maintenance_state.export_worker_shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(10))=>{}}
            }
        });
        while tasks.join_next().await.is_some() {}
    })
}
pub fn stop_service(state: &AppState) {
    state.export_worker_shutdown.cancel();
}
