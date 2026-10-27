use crate::common::{error::AppError, validation};
pub use crate::modules::codegen::dto::request_id;
pub use crate::modules::codegen::dto::{MAX_VERSION, version};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub label: String,
    pub max_output_tokens: u32,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderInput {
    pub name: String,
    pub profile_id: String,
    pub profile_version: i64,
    pub models: Vec<Model>,
    pub enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderPatch {
    pub version: i64,
    pub name: String,
    pub profile_id: String,
    pub profile_version: i64,
    pub models: Vec<Model>,
    pub enabled: bool,
}
impl ProviderPatch {
    pub fn input(self) -> ProviderInput {
        ProviderInput {
            name: self.name,
            profile_id: self.profile_id,
            profile_version: self.profile_version,
            models: self.models,
            enabled: self.enabled,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    pub version: i64,
}
impl ProviderInput {
    pub fn validate(&self) -> Result<(), AppError> {
        validation::text(&self.name, "上游名称", 80)?;
        if self.name.trim() != self.name {
            return Err(AppError::Validation("上游名称不能包含首尾空白".into()));
        }
        let id = validation::object_id(&self.profile_id)?;
        version(self.profile_version)?;
        if id.to_hex() != self.profile_id || self.models.is_empty() || self.models.len() > 32 {
            return Err(AppError::Validation("批准的模型配置无效".into()));
        }
        let mut names = HashSet::new();
        for model in &self.models {
            validation::text(&model.label, "模型名称", 80)?;
            if model.id.is_empty()
                || model.id.len() > 160
                || !model
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
                || !names.insert(&model.id)
                || !(1..=32768).contains(&model.max_output_tokens)
            {
                return Err(AppError::Validation(
                    "模型标识必须唯一且 Token 上限有效".into(),
                ));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunInput {
    pub request_id: String,
    pub provider_id: String,
    pub provider_version: i64,
    pub model: String,
    pub input: String,
    pub instructions: Option<String>,
    pub max_output_tokens: u32,
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputFormat {
    Requirements,
}
impl OutputFormat {
    pub fn definition(self) -> tri_ai_runtime::http::JsonFormat {
        match self {
            Self::Requirements => tri_ai_runtime::http::JsonFormat {
                name: "tri_admin_requirements_v1".into(),
                schema: tri_workflow_runtime::Analysis::schema(),
            },
        }
    }
}
impl RunInput {
    pub fn validate(&self) -> Result<(), AppError> {
        request_id(&self.request_id)?;
        version(self.provider_version)?;
        if validation::object_id(&self.provider_id)?.to_hex() != self.provider_id
            || self.input.trim().is_empty()
            || self.input.len() > 65536
            || self
                .instructions
                .as_ref()
                .is_some_and(|text| text.len() > 16384)
            || self.model.is_empty()
            || self.model.len() > 160
            || !(1..=32768).contains(&self.max_output_tokens)
        {
            return Err(AppError::Validation("AI 运行输入或限额无效".into()));
        }
        Ok(())
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancel {
    pub request_id: String,
    pub version: i64,
}
