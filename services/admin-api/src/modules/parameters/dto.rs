use crate::common::request::{optional_non_null, optional_nullable};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    String,
    Number,
    Boolean,
    Json,
}
impl ValueType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Json => "json",
        }
    }
}
fn general() -> String {
    "general".into()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateParameter {
    pub name: String,
    pub key: String,
    #[serde(default = "general")]
    pub group: String,
    #[serde(default)]
    pub description: String,
    pub value_type: ValueType,
    pub value: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchParameter {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub key: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub group: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub value_type: Option<ValueType>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub value: Option<Option<Value>>,
}
