use crate::common::{error::AppError, validation};
pub use crate::modules::codegen::dto::{MAX_VERSION, request_id, version};
use serde::{Deserialize, Serialize};
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreatePlan {
    pub request_id: String,
    pub name: String,
    pub plan: tri_workflow_runtime::Plan,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchPlan {
    pub version: i64,
    pub name: String,
    pub plan: tri_workflow_runtime::Plan,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    pub version: i64,
}
pub fn plan(name: &str, plan: &tri_workflow_runtime::Plan) -> Result<(), AppError> {
    validation::text(name, "工作流名称", 80)?;
    if name.chars().any(char::is_control) {
        return Err(AppError::Validation("工作流名称无效".into()));
    }
    plan.validate()
        .map_err(|_| AppError::Validation("工作流步骤、依赖或提示无效".into()))?;
    Ok(())
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    pub request_id: String,
    pub plan_id: String,
    pub plan_version: i64,
    pub provider_id: String,
    pub provider_version: i64,
    pub model: String,
    pub max_output_tokens: u32,
    pub stream: bool,
    pub inputs: tri_workflow_runtime::Inputs,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Analyze {
    pub request_id: String,
    pub provider_id: String,
    pub provider_version: i64,
    pub model: String,
    pub max_output_tokens: u32,
    pub stream: bool,
    pub inputs: tri_workflow_runtime::Inputs,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancel {
    pub request_id: String,
    pub version: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportInput {
    pub request_id: String,
    pub version: i64,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub provider_id: Option<String>,
    pub provider_version: Option<i64>,
    pub model: Option<String>,
    pub max_output_tokens: u32,
    pub stream: bool,
    pub mode: tri_workflow_runtime::Mode,
    pub plan_id: Option<String>,
    pub plan_version: Option<i64>,
}
impl Selection {
    pub fn validate(&self) -> Result<(), AppError> {
        if !(1..=32768).contains(&self.max_output_tokens) {
            return Err(AppError::Validation("Token 上限无效".into()));
        }
        if let Some(id) = &self.provider_id {
            if validation::object_id(id)?.to_hex() != *id
                || self.provider_version.is_none()
                || self
                    .model
                    .as_ref()
                    .is_none_or(|value| value.is_empty() || value.len() > 160)
            {
                return Err(AppError::Validation("模型选择无效".into()));
            }
            version(self.provider_version.unwrap_or(0))?;
        } else if self.provider_version.is_some() || self.model.is_some() {
            return Err(AppError::Validation("模型选择不完整".into()));
        }
        if let Some(id) = &self.plan_id {
            validation::object_id(id)?;
            version(self.plan_version.unwrap_or(0))?;
        } else if self.plan_version.is_some() {
            return Err(AppError::Validation("工作流选择不完整".into()));
        }
        Ok(())
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateSession {
    pub request_id: String,
    pub name: String,
    pub selection: Selection,
    pub draft: Option<tri_workflow_runtime::Inputs>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchSession {
    pub version: i64,
    pub name: String,
    pub selection: Selection,
    pub draft: Option<tri_workflow_runtime::Inputs>,
    pub current_message_id: Option<String>,
    pub current_node: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddMessage {
    pub request_id: String,
    pub version: i64,
    pub parent_message_id: Option<String>,
    pub run_id: String,
}
