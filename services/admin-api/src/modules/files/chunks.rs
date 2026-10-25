use super::{
    cleanup,
    models::{Chunk, Upload},
    storage::{BlobId, BlobStore, VerifiedSource},
    uploads, validate,
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared::{self, Actor},
    state::AppState,
};
use axum::body::Bytes;
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub upload_id: String,
    pub index: i64,
    pub size: i64,
    pub sha256: String,
}
impl Receipt {
    fn new(id: ObjectId, row: Chunk) -> Self {
        Self {
            upload_id: id.to_hex(),
            index: row.index,
            size: row.size,
            sha256: row.sha256,
        }
    }
}

enum Prepared {
    Replay(Receipt),
    Write {
        attempt_id: ObjectId,
        blob_id: ObjectId,
    },
}

fn expected(row: &Upload, index: i64, size: i64) -> Result<(), AppError> {
    if index < 0
        || index >= row.total_chunks
        || size != (row.size - index * row.chunk_size).min(row.chunk_size)
    {
        return Err(AppError::Validation("分片索引或长度无效".into()));
    }
    Ok(())
}

pub async fn preflight(
    database: &Database,
    owner: ObjectId,
    id: ObjectId,
    index: i64,
) -> Result<(), AppError> {
    let row = uploads::get_record(database, owner, id).await?;
    if row.state != "receiving" {
        return Err(AppError::Conflict("上传当前不接收分片".into()));
    }
    if index < 0 || index >= row.total_chunks {
        return Err(AppError::Validation("分片索引无效".into()));
    }
    Ok(())
}

async fn orphan(
    database: &Database,
    owner: ObjectId,
    id: ObjectId,
    attempt: ObjectId,
) -> Result<(), AppError> {
    transaction::run(database, database.clone(), |session, database| {
        async move {
            shared::lock(database, "files", session).await?;
            let row = database
                .collection::<Upload>("file_uploads")
                .find_one(doc! {
                                "_id":id,
                "ownerId":owner})
                .session(&mut *session)
                .await?
                .ok_or_else(|| fail(Failure::NotFound))?;
            let result = database
                .collection::<Document>("file_attempts")
                .update_one(
                    doc! {
                    "_id":attempt,
                    "uploadId":id,
                    "state":{
                    "$ne":"accepted"}
                    },
                    doc! {
                    "$set":{
                    "state":"orphan",
                    "updatedAt":DateTime::now()}
                    },
                )
                .session(&mut *session)
                .await?;
            if result.matched_count == 1 {
                cleanup::schedule(database, &row, session).await?;
            }
            Ok(())
        }
        .boxed()
    })
    .await
}

pub async fn put(
    state: &AppState,
    actor: Actor,
    id: ObjectId,
    index: i64,
    data: Bytes,
    hash: String,
) -> Result<Receipt, AppError> {
    let database = state.require_database()?;
    let size = data.len() as i64;
    let spec = validate::spec(size as u64, &hash)?;
    let hash = spec.sha256().to_owned();
    let owner = actor.id;
    let attempt_id = ObjectId::new();
    let blob_id = ObjectId::new();
    let prepared = transaction::run(
        database,
        (database.clone(), actor.clone(), hash.clone()),
        |session, (database, actor, hash)| {
            async move {
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                    "_id":id,
                    "ownerId":actor.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if row.state != "receiving" {
                    return Err(fail(Failure::Conflict("上传当前不接收分片")));
                }
                expected(&row, index, size)
                    .map_err(|_| fail(Failure::Validation("分片索引或长度无效")))?;
                if let Some(chunk) = database
                    .collection::<Chunk>("file_chunks")
                    .find_one(doc! {
                    "uploadId":id,
                    "index":index})
                    .session(&mut *session)
                    .await?
                {
                    if chunk.size != size || chunk.sha256 != *hash {
                        return Err(fail(Failure::Conflict("此分片已接收不同内容")));
                    }
                    if chunk.state == "accepted" {
                        return Ok(Prepared::Replay(Receipt::new(id, chunk)));
                    }
                    if chunk.state != "invalid" {
                        return Err(fail(Failure::NotReady));
                    }
                }
                if row.attempt_count >= 4096
                    || database
                        .collection::<Document>("file_attempts")
                        .count_documents(doc! {
                        "uploadId":id,
                        "state":"writing"})
                        .session(&mut *session)
                        .await?
                        >= 8
                {
                    return Err(fail(Failure::RateLimited));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("file_attempts")
                    .insert_one(doc! {
                    "_id":attempt_id,
                    "uploadId":id,
                    "ownerId":actor.id,
                    "storageProfileId":row.storage_profile_id,
                    "blobId":blob_id,
                    "kind":"chunk",
                    "index":index,
                    "size":size,
                    "sha256":&*hash,
                    "state":"writing",
                    "leaseUntil":DateTime::from_millis(now.timestamp_millis()+300000),
                    "createdAt":now,
                    "updatedAt":now})
                    .session(&mut *session)
                    .await?;
                database
                    .collection::<Document>("file_uploads")
                    .update_one(
                        doc! {
                        "_id":id},
                        doc! {
                        "$inc":{
                        "attemptCount":1i64}
                        },
                    )
                    .session(session)
                    .await?;
                Ok(Prepared::Write {
                    attempt_id,
                    blob_id,
                })
            }
            .boxed()
        },
    )
    .await?;
    let (attempt_id, blob_id) = match prepared {
        Prepared::Replay(receipt) => return Ok(receipt),
        Prepared::Write {
            attempt_id,
            blob_id,
        } => (attempt_id, blob_id),
    };
    let io = async {
        let source =
            VerifiedSource::stage(&state.config, data.as_ref(), spec, CancellationToken::new())
                .await?;
        let store = BlobStore::local(state.config.clone(), ".tri-upload-chunks").await?;
        store
            .put(
                BlobId::parse(&blob_id.to_hex())?,
                source,
                CancellationToken::new(),
            )
            .await
    }
    .await;
    if io.is_err() {
        let _ = orphan(database, owner, id, attempt_id).await;
        let _ = cleanup::run(state, owner, id).await;
        return Err(AppError::FileStorageUnavailable);
    }
    let result = transaction::run(
        database,
        (database.clone(), actor, hash),
        |session, (database, actor, hash)| {
            async move {
                shared::lock(database, "files", session).await?;
                let row = database
                    .collection::<Upload>("file_uploads")
                    .find_one(doc! {
                                        "_id":id,
                    "ownerId":actor.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if row.state != "receiving" {
                    return Err(fail(Failure::Conflict("上传已取消或不再接收分片")));
                }
                if let Some(existing) = database
                    .collection::<Chunk>("file_chunks")
                    .find_one(doc! {
                                        "uploadId":id,
                    "index":index})
                    .session(&mut *session)
                    .await?
                {
                    if existing.size != size || existing.sha256 != *hash {
                        return Err(fail(Failure::Conflict("此分片已接收不同内容")));
                    }
                    if existing.state == "accepted" && existing.blob_id != blob_id {
                        database
                            .collection::<Document>("file_attempts")
                            .update_one(
                                doc! {
                                "_id":attempt_id},
                                doc! {
                                "$set":{
                                "state":"orphan",
                                "updatedAt":DateTime::now()}
                                },
                            )
                            .session(&mut *session)
                            .await?;
                        cleanup::schedule(database, &row, session).await?;
                    }
                    if existing.state == "accepted" {
                        return Ok(Receipt::new(id, existing));
                    }
                    if existing.state != "invalid" {
                        return Err(fail(Failure::NotReady));
                    }
                }
                let attempt = database
                    .collection::<Document>("file_attempts")
                    .find_one(doc! {
                                        "_id":attempt_id,
                    "state":"writing"})
                    .session(&mut *session)
                    .await?;
                if attempt.is_none() {
                    return Err(fail(Failure::Conflict("分片写入尝试已失效")));
                }
                let now = DateTime::now();
                database
                    .collection::<Document>("file_chunks")
                    .update_one(doc! {"uploadId":id,"index":index}, doc! {
                        "$set":{"size":size,"sha256":&*hash,"blobId":blob_id,"state":"accepted"},
                        "$setOnInsert":{"createdAt":now}
                    }).upsert(true)
                    .session(&mut *session)
                    .await?;
                database
                    .collection::<Document>("file_attempts")
                    .update_one(
                        doc! {
                        "_id":attempt_id},
                        doc! {
                        "$set":{
                        "state":"accepted",
                        "updatedAt":now}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                database
                    .collection::<Document>("file_uploads")
                    .update_one(
                        doc! {
                        "_id":id},
                        doc! {
                        "$inc":{
                        "receivedCount":1i64,
                        "receivedBytes":size,
                        "version":1i64}
                        ,
                        "$set":{
                        "updatedAt":now}
                        },
                    )
                    .session(&mut *session)
                    .await?;
                shared::audit(database, actor, "uploads.chunk.accept", id, session).await?;
                Ok(Receipt {
                    upload_id: id.to_hex(),
                    index,
                    size,
                    sha256: hash.clone(),
                })
            }
            .boxed()
        },
    )
    .await;
    if result.is_err() {
        let known = database
            .collection::<Document>("file_attempts")
            .find_one(doc! {
            "_id":attempt_id})
            .await?;
        if known.as_ref().and_then(|row| row.get_str("state").ok()) == Some("accepted")
            && let Some(chunk) = database
                .collection::<Chunk>("file_chunks")
                .find_one(doc! {
                                "uploadId":id,
                "index":index,
                "blobId":blob_id})
                .await?
        {
            return Ok(Receipt::new(id, chunk));
        }
        let _ = orphan(database, owner, id, attempt_id).await;
    }
    let _ = cleanup::run(state, owner, id).await;
    result
}
