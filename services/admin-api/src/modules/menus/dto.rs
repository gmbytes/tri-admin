use crate::common::request::{optional_non_null, optional_nullable};
use serde::Deserialize;
fn enabled() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateMenu {
    pub label: String,
    pub path: String,
    pub view_key: Option<String>,
    pub icon: Option<String>,
    pub permission: Option<String>,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub sort: i32,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchMenu {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub label: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub path: Option<String>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub view_key: Option<Option<String>>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub icon: Option<Option<String>>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub permission: Option<Option<String>>,
    #[serde(default, deserialize_with = "optional_nullable")]
    pub parent_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub sort: Option<i32>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub enabled: Option<bool>,
}
