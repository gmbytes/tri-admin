use crate::common::{model::DataScope, request::optional_non_null};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct RoleChoice {
    pub id: String,
    pub name: String,
    pub slug: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRole {
    pub name: String,
    pub slug: String,
    pub permissions: Vec<String>,
    #[serde(default)]
    pub data_scope: DataScope,
    #[serde(default)]
    pub data_scope_role_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchRole {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub slug: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub permissions: Option<Vec<String>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub data_scope: Option<DataScope>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub data_scope_role_ids: Option<Vec<String>>,
}
