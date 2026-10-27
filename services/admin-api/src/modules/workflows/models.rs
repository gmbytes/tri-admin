use crate::{common::error::AppError, modules::ai::payload::Blob};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
use tri_workflow_runtime::{Mode, Plan};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub creation_fingerprint: String,
    pub name: String,
    pub mode: Mode,
    pub step_count: u32,
    pub version: i64,
    pub revision_id: ObjectId,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub id: String,
    pub owner_id: String,
    pub request_id: String,
    pub name: String,
    pub mode: Mode,
    pub step_count: u32,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}
impl PlanRecord {
    pub fn view(self) -> Result<PlanView, AppError> {
        let time = crate::modules::extensions::models::time;
        Ok(PlanView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            request_id: self.request_id,
            name: self.name,
            mode: self.mode,
            step_count: self.step_count,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            deleted_at: self.deleted_at.map(time).transpose()?,
        })
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub plan_id: ObjectId,
    pub owner_id: ObjectId,
    pub version: i64,
    pub name: String,
    pub sha256: String,
    pub blob: Blob,
    pub created_at: DateTime,
    pub expires_at: Option<DateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionView {
    pub plan_id: String,
    pub version: i64,
    pub name: String,
    pub sha256: String,
    pub plan: Plan,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub id: &'static str,
    pub title: &'static str,
    pub plan: Plan,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Child {
    pub id: String,
    pub request_id: String,
    pub cancel_request_id: String,
    pub ai_id: ObjectId,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub title: String,
    pub plan: Plan,
    pub inputs: tri_workflow_runtime::Inputs,
    pub children: Vec<Child>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub state: tri_workflow_runtime::Outcome,
    pub ai_run_id: Option<ObjectId>,
    pub error_code: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub payload_sha256: String,
    pub plan_id: Option<ObjectId>,
    pub plan_version: Option<i64>,
    pub mode: Mode,
    pub provider_id: ObjectId,
    pub provider_version: i64,
    pub profile_id: ObjectId,
    pub profile_version: i64,
    pub binding_fingerprint: String,
    pub model: String,
    pub max_output_tokens: u32,
    pub stream: bool,
    pub state: tri_workflow_runtime::Outcome,
    pub nodes: Vec<Node>,
    pub version: i64,
    pub generation: i64,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<DateTime>,
    pub next_run_at: DateTime,
    pub cancel_requested: bool,
    pub cancel_request_id: Option<String>,
    pub cancel_fingerprint: Option<String>,
    pub error_code: Option<String>,
    pub execution_deadline: DateTime,
    pub expires_at: DateTime,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub finished_at: Option<DateTime>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPayload {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub blob: Blob,
    pub expires_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeView {
    pub id: String,
    pub state: tri_workflow_runtime::Outcome,
    pub ai_run_id: Option<String>,
    pub error_code: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: String,
    pub owner_id: String,
    pub request_id: String,
    pub plan_id: Option<String>,
    pub plan_version: Option<i64>,
    pub mode: Mode,
    pub state: tri_workflow_runtime::Outcome,
    pub version: i64,
    pub completed_steps: usize,
    pub step_count: usize,
    pub nodes: Vec<NodeView>,
    pub cancel_requested: bool,
    pub cancel_confirmed: bool,
    pub error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}
impl Run {
    pub fn view(self) -> Result<RunView, AppError> {
        let time = crate::modules::extensions::models::time;
        let completed_steps = self
            .nodes
            .iter()
            .filter(|node| node.state == tri_workflow_runtime::Outcome::Succeeded)
            .count();
        Ok(RunView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            request_id: self.request_id,
            plan_id: self.plan_id.map(|id| id.to_hex()),
            plan_version: self.plan_version,
            mode: self.mode,
            state: self.state,
            version: self.version,
            completed_steps,
            step_count: self.nodes.len(),
            nodes: self
                .nodes
                .into_iter()
                .map(|node| NodeView {
                    id: node.id,
                    state: node.state,
                    ai_run_id: node.ai_run_id.map(|id| id.to_hex()),
                    error_code: node.error_code,
                })
                .collect(),
            cancel_requested: self.cancel_requested,
            cancel_confirmed: self.state == tri_workflow_runtime::Outcome::Cancelled,
            error_code: self.error_code,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            finished_at: self.finished_at.map(time).transpose()?,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepContent {
    pub id: String,
    pub title: String,
    pub state: tri_workflow_runtime::Outcome,
    pub prompt: String,
    pub text: String,
    pub analysis: Option<tri_workflow_runtime::Analysis>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunContent {
    pub version: i64,
    pub title: String,
    pub inputs: tri_workflow_runtime::Inputs,
    pub plan: Plan,
    pub steps: Vec<StepContent>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub run_id: ObjectId,
    pub run_version: i64,
    pub size: i64,
    pub sha256: String,
    pub file_name: String,
    pub expires_at: DateTime,
    pub created_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportView {
    pub id: String,
    pub request_id: String,
    pub run_id: String,
    pub run_version: i64,
    pub size: i64,
    pub sha256: String,
    pub file_name: String,
    pub expires_at: String,
    pub created_at: String,
}
impl ExportRecord {
    pub fn view(self) -> Result<ExportView, AppError> {
        let time = crate::modules::extensions::models::time;
        Ok(ExportView {
            id: self.id.to_hex(),
            request_id: self.request_id,
            run_id: self.run_id.to_hex(),
            run_version: self.run_version,
            size: self.size,
            sha256: self.sha256,
            file_name: self.file_name,
            expires_at: time(self.expires_at)?,
            created_at: time(self.created_at)?,
        })
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub creation_fingerprint: String,
    pub name: String,
    pub version: i64,
    pub payload_sha256: String,
    pub current_message_id: Option<ObjectId>,
    pub message_count: u32,
    pub expires_at: DateTime,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionData {
    pub selection: super::dto::Selection,
    pub draft: Option<tri_workflow_runtime::Inputs>,
    pub current_node: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: String,
    pub owner_id: String,
    pub request_id: String,
    pub name: String,
    pub version: i64,
    pub current_message_id: Option<String>,
    pub message_count: u32,
    pub expires_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}
impl Session {
    pub fn view(self) -> Result<SessionView, AppError> {
        let time = crate::modules::extensions::models::time;
        Ok(SessionView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            request_id: self.request_id,
            name: self.name,
            version: self.version,
            current_message_id: self.current_message_id.map(|id| id.to_hex()),
            message_count: self.message_count,
            expires_at: time(self.expires_at)?,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            deleted_at: self.deleted_at.map(time).transpose()?,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContent {
    pub version: i64,
    pub selection: super::dto::Selection,
    pub draft: Option<tri_workflow_runtime::Inputs>,
    pub current_message_id: Option<String>,
    pub current_node: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub session_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub parent_message_id: Option<ObjectId>,
    pub run_id: ObjectId,
    pub session_version: i64,
    pub created_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub id: String,
    pub session_id: String,
    pub request_id: String,
    pub parent_message_id: Option<String>,
    pub run_id: String,
    pub session_version: i64,
    pub created_at: String,
}
impl Message {
    pub fn view(self) -> Result<MessageView, AppError> {
        Ok(MessageView {
            id: self.id.to_hex(),
            session_id: self.session_id.to_hex(),
            request_id: self.request_id,
            parent_message_id: self.parent_message_id.map(|id| id.to_hex()),
            run_id: self.run_id.to_hex(),
            session_version: self.session_version,
            created_at: crate::modules::extensions::models::time(self.created_at)?,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageContent {
    pub message: MessageView,
    pub run: RunContent,
}
