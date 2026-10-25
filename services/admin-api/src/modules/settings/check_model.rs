use super::{
    dto::Kind,
    models::{Profile, time},
};
use crate::{
    common::{canonical, error::AppError},
    config::Config,
};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub profile_id: ObjectId,
    pub profile_version: i64,
    pub actor_id: ObjectId,
    pub kind: Kind,
    pub fingerprint: String,
    pub success: bool,
    pub scope: String,
    pub error_code: Option<String>,
    pub upstream_status: Option<i32>,
    pub duration_ms: i64,
    pub created_at: DateTime,
    pub completed_at: DateTime,
    pub request_id: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckView {
    pub id: String,
    pub profile_id: String,
    pub profile_version: i64,
    pub actor_id: String,
    pub kind: Kind,
    pub success: bool,
    pub scope: String,
    pub error_code: Option<String>,
    pub upstream_status: Option<i32>,
    pub duration_ms: i64,
    pub started_at: String,
    pub completed_at: String,
    pub request_id: Option<String>,
    pub is_current: bool,
}
pub fn fingerprint(row: &Profile, config: &Config) -> Result<String, AppError> {
    Ok(canonical::checksum(canonical::json(&serde_json::json!({"id":row.id.to_hex(),"version":row.version,"kind":row.kind,"configuration":row.configuration_json,"secret":row.secret_box,"enabled":row.enabled,"process":config.connection_epoch()}))?.as_bytes()))
}
impl CheckRecord {
    pub fn view(&self, current: bool) -> Result<CheckView, AppError> {
        Ok(CheckView {
            id: self.id.to_hex(),
            profile_id: self.profile_id.to_hex(),
            profile_version: self.profile_version,
            actor_id: self.actor_id.to_hex(),
            kind: self.kind,
            success: self.success,
            scope: self.scope.clone(),
            error_code: self.error_code.clone(),
            upstream_status: self.upstream_status,
            duration_ms: self.duration_ms,
            started_at: time(self.created_at)?,
            completed_at: time(self.completed_at)?,
            request_id: self.request_id.clone(),
            is_current: current,
        })
    }
}
