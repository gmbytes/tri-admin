use super::{
    cleanup, jobs,
    models::{Chunk, Upload},
    storage::{BlobId, BlobSpec, BlobStore, StorageError},
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
use futures_util::{FutureExt, StreamExt, TryStreamExt};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    #[serde(rename = "_id")]
    id: ObjectId,
    upload_id: ObjectId,
    blob_id: ObjectId,
    index: i64,
    size: i64,
    sha256: String,
}
#[derive(Default, Serialize)]
pub struct RecoveryView {
    pub recovered: u64,
    pub retired: u64,
    pub unknown: u64,
}
fn eligible(now: DateTime, force: bool) -> Document {
    let mut unknown = doc! {"state":"orphanUnknown"};
    if !force {
        unknown.insert("nextRunAt", doc! {"$lte":now});
    }
    doc! {"kind":"chunk","$or":[{"state":"writing","leaseUntil":{"$lte":now}},{"state":"recovering","recoveryUntil":{"$lte":now}},unknown]}
}
pub async fn recover(
    state: &AppState,
    owner: ObjectId,
    id: ObjectId,
) -> Result<RecoveryView, AppError> {
    uploads::get_record(state.require_database()?, owner, id).await?;
    run(state, Some(id), true).await
}
pub(crate) async fn background(state: &AppState) -> Result<RecoveryView, AppError> {
    run(state, None, false).await
}
async fn run(
    state: &AppState,
    upload: Option<ObjectId>,
    force: bool,
) -> Result<RecoveryView, AppError> {
    let database = state.require_database()?;
    let mut filter = eligible(DateTime::now(), force);
    if let Some(id) = upload {
        filter.insert("uploadId", id);
    }
    let candidates = database
        .collection::<Candidate>("file_attempts")
        .find(filter)
        .sort(doc! {"nextRunAt":1,"createdAt":1,"_id":1})
        .limit(32)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let mut result = RecoveryView::default();
    let mut parents = std::collections::HashMap::new();
    let store = if candidates.is_empty() {
        None
    } else {
        Some(
            BlobStore::local(state.config.clone(), ".tri-upload-chunks")
                .await
                .map_err(|_| AppError::FileStorageUnavailable)?,
        )
    };
    for candidate in candidates {
        let token = uuid::Uuid::new_v4().to_string();
        let claimed=transaction::run(database,(database.clone(),candidate.clone(),token.clone()),|session,(database,candidate,token)|{
            async move{
                shared::lock(database,"files",session).await?;let mut filter=eligible(DateTime::now(),force);filter.insert("_id",candidate.id);
                if database.collection::<Document>("file_attempts").find_one(filter).session(&mut *session).await?.is_none(){return Ok(false)}
                let parent=database.collection::<Upload>("file_uploads").find_one(doc!{"_id":candidate.upload_id}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotReady))?;
                if database.collection::<Document>("file_chunks").find_one(doc!{"uploadId":parent.id,"blobId":candidate.blob_id,"state":"accepted"}).session(&mut *session).await?.is_some(){
                    database.collection::<Document>("file_attempts").update_one(doc!{"_id":candidate.id},doc!{"$set":{"state":"accepted","updatedAt":DateTime::now()}}).session(&mut *session).await?;
                    if matches!(parent.state.as_str(),"completed"|"cancelled"){cleanup::schedule(database,&parent,session).await?;}return Ok(false)
                }
                let now=DateTime::now();database.collection::<Document>("file_attempts").update_one(doc!{"_id":candidate.id},doc!{"$set":{"state":"recovering","recoveryToken":&*token,"recoveryUntil":DateTime::from_millis(now.timestamp_millis()+60000),"updatedAt":now}}).session(session).await?;Ok(true)
            }.boxed()
        }).await?;
        if !claimed {
            continue;
        }
        let spec = BlobSpec::new(candidate.size as u64, &candidate.sha256)
            .map_err(|_| AppError::NotReady)?;
        let observed = match store
            .as_ref()
            .ok_or(AppError::Internal)?
            .read(
                BlobId::parse(&candidate.blob_id.to_hex()).map_err(|_| AppError::NotReady)?,
                spec,
                CancellationToken::new(),
            )
            .await
        {
            Ok(mut stream) => {
                let mut status = 1i32;
                while let Some(chunk) = stream.next().await {
                    if let Err(error) = chunk {
                        status = if error == StorageError::IntegrityMismatch {
                            2
                        } else {
                            0
                        };
                        break;
                    }
                }
                status
            }
            Err(StorageError::IntegrityMismatch) => 2,
            Err(_) => 0,
        };
        let outcome=transaction::run(database,(database.clone(),candidate.clone(),token),|session,(database,candidate,token)|{
            async move{
                shared::lock(database,"rbac",session).await?;shared::lock(database,"files",session).await?;
                if database.collection::<Document>("file_attempts").find_one(doc!{"_id":candidate.id,"state":"recovering","recoveryToken":&*token}).session(&mut *session).await?.is_none(){return Ok((0u8,None))}
                let parent=database.collection::<Upload>("file_uploads").find_one(doc!{"_id":candidate.upload_id}).session(&mut *session).await?.ok_or_else(||fail(Failure::NotReady))?;
                let existing=database.collection::<Chunk>("file_chunks").find_one(doc!{"uploadId":parent.id,"index":candidate.index}).session(&mut *session).await?;
                let current=existing.as_ref().is_some_and(|chunk|chunk.state=="accepted"&&chunk.blob_id==candidate.blob_id);
                let expected=candidate.index>=0&&candidate.index<parent.total_chunks&&candidate.size==(parent.size-candidate.index*parent.chunk_size).min(parent.chunk_size);
                let replaceable=existing.as_ref().is_none_or(|chunk|chunk.state=="invalid"&&chunk.sha256==candidate.sha256&&chunk.size==candidate.size);
                let adopt=observed==1&&parent.state=="receiving"&&expected&&replaceable&&jobs::allowed(database,parent.owner_id,session).await?;
                let now=DateTime::now();
                if current||adopt{
                    database.collection::<Document>("file_attempts").update_one(doc!{"_id":candidate.id},doc!{"$set":{"state":"accepted","recoveryToken":null,"updatedAt":now}}).session(&mut *session).await?;
                    if adopt{
                        database.collection::<Document>("file_chunks").update_one(doc!{"uploadId":parent.id,"index":candidate.index},doc!{"$set":{"size":candidate.size,"sha256":&candidate.sha256,"blobId":candidate.blob_id,"state":"accepted"},"$setOnInsert":{"createdAt":now}}).upsert(true).session(&mut *session).await?;
                        database.collection::<Document>("file_uploads").update_one(doc!{"_id":parent.id},doc!{"$inc":{"receivedCount":1i64,"receivedBytes":candidate.size,"version":1i64},"$set":{"updatedAt":now,"lastError":null}}).session(&mut *session).await?;
                        database.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"administration","action":"uploads.chunk.recover","actorId":parent.owner_id,"targetId":parent.id,"outcome":"success","index":candidate.index,"createdAt":now})).session(&mut *session).await?;
                    }
                    return Ok((if adopt{1}else{0},Some(parent.owner_id)))
                }
                database.collection::<Document>("file_attempts").update_one(doc!{"_id":candidate.id},doc!{"$set":{"state":if observed==0{"orphanUnknown"}else{"orphan"},"recoveryToken":null,"nextRunAt":DateTime::from_millis(now.timestamp_millis()+30000),"updatedAt":now}}).session(&mut *session).await?;
                cleanup::schedule(database,&parent,session).await?;Ok((if observed==0{3}else{2},Some(parent.owner_id)))
            }.boxed()
        }).await?;
        match outcome.0 {
            1 => result.recovered += 1,
            2 => result.retired += 1,
            3 => result.unknown += 1,
            _ => {}
        }
        if let Some(owner) = outcome.1 {
            parents.insert(candidate.upload_id, owner);
        }
    }
    for (id, owner) in parents {
        let _ = cleanup::run(state, owner, id).await;
    }
    Ok(result)
}
