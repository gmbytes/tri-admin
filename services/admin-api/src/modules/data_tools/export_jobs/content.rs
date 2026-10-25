use super::{authorization, models::JobState, service};
use crate::{
    common::error::AppError,
    modules::{
        files::storage::{BlobId, BlobSpec, BlobStore},
        rbac::shared::Actor,
    },
    state::AppState,
};
use axum::{
    body::Body,
    http::{HeaderValue, header},
    response::Response,
};
use futures_util::TryStreamExt;
use mongodb::bson::{DateTime, oid::ObjectId};
use tokio_util::sync::CancellationToken;
pub async fn download(state: &AppState, actor: &Actor, id: ObjectId) -> Result<Response, AppError> {
    let database = state.require_database()?;
    let job = service::get_record(database, actor, id).await?;
    let now = DateTime::now();
    if job.meta.state != JobState::Completed
        || job.meta.expires_at.is_none_or(|expires| expires <= now)
    {
        return Err(AppError::Conflict("导出结果尚未完成或已过期".into()));
    }
    authorization::verify_user_manifest(database, &job, actor).await?;
    let file = job.meta.clone().view(now)?.file.ok_or(AppError::NotReady)?;
    let output = job.meta.output.ok_or(AppError::NotReady)?;
    let spec = BlobSpec::new(file.size, &file.sha256).map_err(|_| AppError::NotReady)?;
    let store = BlobStore::from_profile(database, state.config.clone(), job.storage_profile_id)
        .await
        .map_err(|_| AppError::FileStorageUnavailable)?;
    let stream = store
        .read(
            BlobId::parse(&output.blob_id.to_hex()).map_err(|_| AppError::NotReady)?,
            spec,
            CancellationToken::new(),
        )
        .await
        .map_err(|_| AppError::FileStorageUnavailable)?;
    let reporter = crate::common::observability::stream_reporter(database);
    let stream = stream.inspect_err(move |_| {
        if let Some(reporter) = &reporter {
            reporter.record("EXPORT_CONTENT_INTEGRITY");
        }
    });
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&file.size.to_string()).map_err(|_| AppError::NotReady)?,
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", file.name))
            .map_err(|_| AppError::NotReady)?,
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", file.sha256)).map_err(|_| AppError::NotReady)?,
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}
