use crate::common::request::optional_non_null;
use serde::Deserialize;
fn active() -> String {
    "active".into()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateUser {
    pub username: String,
    pub display_name: String,
    pub password: String,
    pub role_ids: Vec<String>,
    #[serde(default = "active")]
    pub status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchUser {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub username: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub display_name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub role_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub status: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordInput {
    pub version: i64,
    pub password: String,
}
