use crate::common::request::optional_non_null;
use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDictionary {
    pub name: String,
    pub key: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchDictionary {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub key: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub enabled: Option<bool>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateItem {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub sort: i32,
    #[serde(default = "yes")]
    pub enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchItem {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub label: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub value: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub sort: Option<i32>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub enabled: Option<bool>,
}
