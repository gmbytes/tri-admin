use crate::common::{error::AppError, validation};
use serde::Deserialize;
use tri_resource_runtime::schema::Schema;
pub const MAX_VERSION: i64 = 9_007_199_254_740_990;
pub fn version(value: i64) -> Result<(), AppError> {
    if (1..=MAX_VERSION).contains(&value) {
        Ok(())
    } else {
        Err(AppError::Validation("版本号无效".into()))
    }
}
pub fn request_id(value: &str) -> Result<(), AppError> {
    if uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value) {
        Ok(())
    } else {
        Err(AppError::Validation("请求标识必须为规范 UUID".into()))
    }
}
pub fn schema(value: &Schema) -> Result<(), AppError> {
    value
        .validate()
        .map_err(|error| AppError::Validation(error.to_string()))
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDefinition {
    pub request_id: String,
    pub schema: Schema,
}
impl CreateDefinition {
    pub fn validate(&self) -> Result<(), AppError> {
        request_id(&self.request_id)?;
        schema(&self.schema)
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDefinition {
    pub version: i64,
    pub schema: Schema,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    pub version: i64,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Generate {
    pub request_id: String,
    pub definition_id: String,
    pub definition_version: i64,
}
impl Generate {
    pub fn validate(&self) -> Result<(), AppError> {
        request_id(&self.request_id)?;
        validation::object_id(&self.definition_id)?;
        version(self.definition_version)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub path: String,
}
fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryQuery {
    #[serde(default = "first")]
    pub page: u64,
    #[serde(default = "size")]
    pub page_size: u64,
    pub search: Option<String>,
    pub definition_id: Option<String>,
}
