use super::dto::Model;
use crate::common::error::AppError;
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub profile_id: ObjectId,
    pub profile_version: i64,
    pub binding_fingerprint: String,
    pub models: Vec<Model>,
    pub enabled: bool,
    pub version: i64,
    pub created_by: ObjectId,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub profile_id: String,
    pub profile_version: i64,
    pub binding_current: bool,
    pub models: Vec<Model>,
    pub enabled: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl Provider {
    pub fn view(self, current: bool) -> Result<ProviderView, AppError> {
        Ok(ProviderView {
            id: self.id.to_hex(),
            name: self.name,
            profile_id: self.profile_id.to_hex(),
            profile_version: self.profile_version,
            binding_current: current,
            models: self.models,
            enabled: self.enabled,
            version: self.version,
            created_at: crate::modules::extensions::models::time(self.created_at)?,
            updated_at: crate::modules::extensions::models::time(self.updated_at)?,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovedModel {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_version: i64,
    pub id: String,
    pub label: String,
    pub max_output_tokens: u32,
}
#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub provider_id: ObjectId,
    pub provider_version: i64,
    pub profile_id: ObjectId,
    pub profile_version: i64,
    pub binding_fingerprint: String,
    pub model: String,
    pub max_output_tokens: u32,
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_origin: Option<WorkflowOrigin>,
    pub state: RunState,
    pub submission_started: bool,
    pub has_response: bool,
    pub cancel_requested: bool,
    pub cancel_request_id: Option<String>,
    pub cancel_fingerprint: Option<String>,
    pub cancel_sent: bool,
    pub response_status: Option<tri_ai_runtime::Status>,
    pub error_code: Option<String>,
    pub version: i64,
    pub generation: i64,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<DateTime>,
    pub next_run_at: DateTime,
    pub execution_deadline: DateTime,
    pub expires_at: DateTime,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub finished_at: Option<DateTime>,
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowOrigin {
    pub run_id: ObjectId,
    pub step_id: String,
    pub payload_sha256: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: String,
    pub owner_id: String,
    pub request_id: String,
    pub provider_id: String,
    pub provider_version: i64,
    pub model: String,
    pub state: RunState,
    pub version: i64,
    pub may_have_been_submitted: bool,
    pub has_response: bool,
    pub cancel_requested: bool,
    pub cancel_request_id: Option<String>,
    pub cancel_confirmed: bool,
    pub response_status: Option<tri_ai_runtime::Status>,
    pub error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
    pub expires_at: String,
}
impl Run {
    pub fn view(self) -> Result<RunView, AppError> {
        let time = crate::modules::extensions::models::time;
        Ok(RunView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            request_id: self.request_id,
            provider_id: self.provider_id.to_hex(),
            provider_version: self.provider_version,
            model: self.model,
            state: self.state,
            version: self.version,
            may_have_been_submitted: self.submission_started,
            has_response: self.has_response,
            cancel_requested: self.cancel_requested,
            cancel_request_id: self.cancel_request_id,
            cancel_confirmed: self.response_status == Some(tri_ai_runtime::Status::Cancelled)
                || self.state == RunState::Cancelled && !self.submission_started,
            response_status: self.response_status,
            error_code: self.error_code,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            finished_at: self.finished_at.map(time).transpose()?,
            expires_at: time(self.expires_at)?,
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivateRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub blob: super::payload::Blob,
    pub expires_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    pub input: String,
    pub instructions: Option<String>,
    pub text: String,
    pub sha256: String,
    pub response_status: Option<tri_ai_runtime::Status>,
    pub refused: bool,
    pub sequence: Option<String>,
}
