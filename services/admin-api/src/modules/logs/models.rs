use crate::common::error::AppError;
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
pub fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::Internal)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub kind: String,
    pub action: String,
    pub outcome: String,
    pub created_at: DateTime,
    pub actor_id: Option<ObjectId>,
    pub target_id: Option<ObjectId>,
    pub username: Option<String>,
    pub request_id: Option<String>,
    pub method: Option<String>,
    pub route: Option<String>,
    pub ip: Option<String>,
    pub status: Option<i64>,
    pub duration_ms: Option<i64>,
    pub error_code: Option<String>,
    pub message: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogView {
    pub id: String,
    pub kind: String,
    pub action: String,
    pub outcome: String,
    pub created_at: String,
    pub actor_id: Option<String>,
    pub actor_name: Option<String>,
    pub target_id: Option<String>,
    pub username: Option<String>,
    pub request_id: Option<String>,
    pub method: Option<String>,
    pub route: Option<String>,
    pub ip: Option<String>,
    pub status: Option<i64>,
    pub duration_ms: Option<i64>,
    pub error_code: Option<String>,
    pub message: Option<String>,
}
impl Record {
    pub fn view(self, actor_name: Option<String>) -> Result<LogView, AppError> {
        Ok(LogView {
            id: self.id.to_hex(),
            kind: self.kind,
            action: self.action,
            outcome: self.outcome,
            created_at: time(self.created_at)?,
            actor_id: self.actor_id.map(ObjectId::to_hex),
            actor_name,
            target_id: self.target_id.map(ObjectId::to_hex),
            username: self.username,
            request_id: self.request_id,
            method: self.method,
            route: self.route,
            ip: self.ip,
            status: self.status,
            duration_ms: self.duration_ms,
            error_code: self.error_code,
            message: self.message,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResult {
    pub request_id: String,
    pub kind: super::dto::Kind,
    pub before: String,
    pub deleted_count: u64,
    pub has_more: bool,
    pub completed_at: String,
}
