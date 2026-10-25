use super::{
    models::{Attempt, Upload},
    storage::{BlobId, BlobStore},
    uploads,
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared,
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
pub async fn schedule(
    database: &Database,
    row: &Upload,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let now = DateTime::now();
    database.collection::<Document>("file_cleanup_tasks").update_one(doc!{"_id":row.id},doc!{"$set":{"state":"pending","nextRunAt":now,"updatedAt":now},"$setOnInsert":{"uploadId":row.id,"storageProfileId":row.storage_profile_id,"createdAt":now}}).upsert(true).session(&mut *session).await?;
    database
        .collection::<Document>("file_uploads")
        .update_one(doc! {"_id":row.id}, doc! {"$set":{"cleanupPending":true}})
        .session(session)
        .await?;
    Ok(())
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupView {
    pub upload_id: String,
    pub processed: u64,
    pub pending: u64,
    pub complete: bool,
}
pub async fn run(state: &AppState, owner: ObjectId, id: ObjectId) -> Result<CleanupView, AppError> {
    run_as(state, owner, id, owner).await
}
fn allowed(parent: &str, kind: &str) -> Vec<&'static str> {
    match kind {
        "chunk" if matches!(parent, "cancelled" | "completed") => {
            vec!["accepted", "orphan", "cleaning"]
        }
        "chunk" | "publish" => vec!["orphan", "cleaning"],
        _ => vec![],
    }
}
fn candidates(parent: &str, id: ObjectId) -> Document {
    doc! {"uploadId":id,"$or":[
        {"kind":"chunk","state":{"$in":allowed(parent,"chunk")}},
        {"kind":"publish","state":{"$in":allowed(parent,"publish")}}
    ]}
}
pub(crate) async fn run_as(
    state: &AppState,
    owner: ObjectId,
    id: ObjectId,
    initiator: ObjectId,
) -> Result<CleanupView, AppError> {
    let database = state.require_database()?;
    let row = uploads::get_record(database, owner, id).await?;
    let candidates = database
        .collection::<Attempt>("file_attempts")
        .find(candidates(&row.state, id))
        .sort(doc! {"createdAt":1,"_id":1})
        .limit(100)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let mut processed = 0u64;
    let mut chunk_store = None;
    for attempt in candidates {
        let claimed = transaction::run(
            database,
            (database.clone(), attempt.id,attempt.kind.clone(),attempt.blob_id),
            |session, (database, attempt_id,kind,blob)| {
                async move {
                    shared::lock(database, "files", session).await?;
                    let parent = database
                        .collection::<Upload>("file_uploads")
                        .find_one(doc! {"_id":id,"ownerId":owner})
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| fail(Failure::NotFound))?;
                    let allowed = allowed(&parent.state,kind);
                    if kind=="publish"&&database.collection::<Document>("files").find_one(doc!{"blobId":*blob,"deletedAt":null}).session(&mut *session).await?.is_some(){return Ok(false)}
                    if kind=="chunk"&&!matches!(parent.state.as_str(),"completed"|"cancelled")&&database.collection::<Document>("file_chunks").find_one(doc!{"blobId":*blob,"state":"accepted"}).session(&mut *session).await?.is_some(){return Ok(false)}
                    let result = database
                        .collection::<Document>("file_attempts")
                        .update_one(
                            doc! {"_id":*attempt_id,"uploadId":id,"kind":&*kind,"state":{"$in":allowed}},
                            doc! {"$set":{"state":"cleaning","updatedAt":DateTime::now()}},
                        )
                        .session(session)
                        .await?;
                    Ok(result.matched_count == 1)
                }
                .boxed()
            },
        )
        .await?;
        if !claimed {
            continue;
        }
        let blob = BlobId::parse(&attempt.blob_id.to_hex()).map_err(|_| AppError::NotReady)?;
        if attempt.kind == "chunk" {
            if chunk_store.is_none() {
                chunk_store = Some(
                    BlobStore::local(state.config.clone(), ".tri-upload-chunks")
                        .await
                        .map_err(|_| AppError::FileStorageUnavailable)?,
                );
            }
            chunk_store
                .as_ref()
                .ok_or(AppError::Internal)?
                .delete(blob, CancellationToken::new())
                .await
                .map_err(|_| AppError::FileStorageUnavailable)?;
        } else if attempt.kind == "publish" {
            BlobStore::from_profile(database, state.config.clone(), attempt.storage_profile_id)
                .await
                .map_err(|_| AppError::FileStorageUnavailable)?
                .delete(blob, CancellationToken::new())
                .await
                .map_err(|_| AppError::FileStorageUnavailable)?;
        } else {
            return Err(AppError::NotReady);
        }
        let result = database
            .collection::<Document>("file_attempts")
            .update_one(
                doc! {"_id":attempt.id,"state":"cleaning"},
                doc! {"$set":{"state":"cleaned","updatedAt":DateTime::now()}},
            )
            .await?;
        processed += result.matched_count;
    }
    let pending = transaction::run(database, database.clone(), |session, database| {
        async move {
            shared::lock(database, "files", session).await?;
            let parent = database
                .collection::<Upload>("file_uploads")
                .find_one(doc! {"_id":id,"ownerId":owner})
                .session(&mut *session)
                .await?
                .ok_or_else(|| fail(Failure::NotFound))?;
            let mut filter = doc! {"uploadId":id};
            if matches!(parent.state.as_str(),"cancelled"|"cancelRequested") {
                filter.insert("state", doc! {"$ne":"cleaned"});
            } else if parent.state=="completed"{
                filter.insert("$or",mongodb::bson::bson!([
                    {"kind":"chunk","state":{"$ne":"cleaned"}},
                    {"state":{"$in":["orphan","cleaning","orphanUnknown"]}}
                ]));
            }else {
                filter.insert("state", doc! {"$in":["orphan","cleaning","orphanUnknown"]});
            }
            let pending = database
                .collection::<Document>("file_attempts")
                .count_documents(filter)
                .session(&mut *session)
                .await?;
            let now = DateTime::now();
            database
                .collection::<Document>("file_cleanup_tasks")
                .update_one(
                    doc! {"_id":id},
                    doc! {"$set":{"state":if pending==0{"done"}else{"pending"},"nextRunAt":DateTime::from_millis(now.timestamp_millis()+if pending==0{0}else{30000}),"updatedAt":now}},
                )
                .session(&mut *session)
                .await?;
            database
                .collection::<Document>("file_uploads")
                .update_one(doc! {"_id":id}, doc! {"$set":{"cleanupPending":pending>0}})
                .session(&mut *session)
                .await?;
            if processed > 0 || parent.cleanup_pending != (pending > 0) {
                database.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"administration","action":"uploads.cleanup","actorId":initiator,"targetId":id,"outcome":"success","processed":processed as i64,"pending":pending as i64,"createdAt":now})).session(session).await?;
            }
            Ok(pending)
        }
        .boxed()
    })
    .await?;
    Ok(CleanupView {
        upload_id: id.to_hex(),
        processed,
        pending,
        complete: pending == 0,
    })
}
