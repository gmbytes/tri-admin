use crate::{
    common::{error::AppError, request::optional_non_null, validation},
    modules::open_access::{
        limits::Policy,
        scopes::{self, Scope},
    },
};
use serde::{Deserialize, Serialize};
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
pub fn name(value: &str, label: &str, max: usize) -> Result<(), AppError> {
    validation::text(value, label, max)?;
    if value.chars().any(char::is_control) {
        return Err(AppError::Validation(format!("{label}不能包含控制字符")));
    }
    Ok(())
}
fn yes() -> bool {
    true
}
fn minute() -> u32 {
    600
}
fn daily() -> u32 {
    100000
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateApplication {
    pub request_id: String,
    pub name: String,
    pub code: String,
    pub allowed_scopes: Vec<Scope>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    #[serde(default = "minute")]
    pub minute_limit: u32,
    #[serde(default = "daily")]
    pub daily_quota: u32,
}
impl CreateApplication {
    pub fn validate(mut self) -> Result<Self, AppError> {
        request_id(&self.request_id)?;
        self.name = self.name.trim().to_owned();
        name(&self.name, "应用名称", 80)?;
        self.code = self.code.trim().to_ascii_lowercase();
        if !(3..=64).contains(&self.code.len())
            || !self
                .code
                .bytes()
                .next()
                .is_some_and(|v| v.is_ascii_lowercase())
            || !self
                .code
                .bytes()
                .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || b"._-".contains(&v))
        {
            return Err(AppError::Validation("应用 code 格式无效".into()));
        }
        self.allowed_scopes = scopes::normalize(self.allowed_scopes, false)?;
        let policy = Policy {
            allowed_ips: self.allowed_ips,
            minute_limit: self.minute_limit,
            daily_quota: self.daily_quota,
        }
        .normalize()?;
        self.allowed_ips = policy.allowed_ips;
        Ok(self)
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchApplication {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub allowed_scopes: Option<Vec<Scope>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub allowed_ips: Option<Vec<String>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub minute_limit: Option<u32>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub daily_quota: Option<u32>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RotateSecret {
    pub request_id: String,
    pub version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    pub version: i64,
}
#[derive(Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Disabled,
}
fn active() -> UserStatus {
    UserStatus::Active
}
fn profile() -> Vec<Scope> {
    vec![Scope::ProfileRead]
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateUser {
    pub username: String,
    pub display_name: String,
    pub password: String,
    #[serde(default = "profile")]
    pub scopes: Vec<Scope>,
    #[serde(default = "active")]
    pub status: UserStatus,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchUser {
    pub version: i64,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub username: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub display_name: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub scopes: Option<Vec<Scope>>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub status: Option<UserStatus>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordInput {
    pub version: i64,
    pub password: String,
}
