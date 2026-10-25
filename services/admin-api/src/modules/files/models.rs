use crate::common::error::AppError;
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
pub const CHUNK_SIZE: u64 = 4 * 1024 * 1024;
pub fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::Internal)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub code: String,
    pub description: String,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryView {
    pub id: String,
    pub name: String,
    pub code: String,
    pub description: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl Category {
    pub fn view(self) -> Result<CategoryView, AppError> {
        Ok(CategoryView {
            id: self.id.to_hex(),
            name: self.name,
            code: self.code,
            description: self.description,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub request_hash: String,
    pub name: String,
    pub media_type: String,
    pub size: i64,
    pub sha256: String,
    pub category_id: Option<ObjectId>,
    pub storage_profile_id: ObjectId,
    pub state: String,
    pub chunk_size: i64,
    pub total_chunks: i64,
    pub received_count: i64,
    pub received_bytes: i64,
    pub attempt_count: i64,
    pub cleanup_pending: bool,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadView {
    pub id: String,
    pub owner_id: String,
    pub request_id: String,
    pub name: String,
    pub media_type: String,
    pub size: i64,
    pub sha256: String,
    pub category_id: Option<String>,
    pub storage_profile_id: String,
    pub state: String,
    pub chunk_size: i64,
    pub total_chunks: i64,
    pub received_count: i64,
    pub received_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_indices: Option<Vec<i64>>,
    pub cleanup_pending: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl Upload {
    pub fn view(self, received_indices: Option<Vec<i64>>) -> Result<UploadView, AppError> {
        Ok(UploadView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            request_id: self.request_id,
            name: self.name,
            media_type: self.media_type,
            size: self.size,
            sha256: self.sha256,
            category_id: self.category_id.map(ObjectId::to_hex),
            storage_profile_id: self.storage_profile_id.to_hex(),
            state: self.state,
            chunk_size: self.chunk_size,
            total_chunks: self.total_chunks,
            received_count: self.received_count,
            received_bytes: self.received_bytes,
            received_indices,
            cleanup_pending: self.cleanup_pending,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub index: i64,
    pub size: i64,
    pub sha256: String,
    pub blob_id: ObjectId,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub blob_id: ObjectId,
}
