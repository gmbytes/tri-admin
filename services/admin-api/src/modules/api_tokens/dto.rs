use crate::{common::error::AppError, modules::open_access::limits::Policy};
use serde::{Deserialize, Serialize};
fn days() -> u32 {
    30
}
fn minute() -> u32 {
    600
}
fn daily() -> u32 {
    100000
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateToken {
    pub request_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    #[serde(default = "days")]
    pub expires_in_days: u32,
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    #[serde(default = "minute")]
    pub minute_limit: u32,
    #[serde(default = "daily")]
    pub daily_quota: u32,
}
impl CreateToken {
    pub fn validate(mut self) -> Result<Self, AppError> {
        let id = uuid::Uuid::parse_str(&self.request_id)
            .map_err(|_| AppError::Validation("请求标识必须是规范 UUID".into()))?;
        self.name = self.name.trim().into();
        if id.to_string() != self.request_id
            || self.name.is_empty()
            || self.name.chars().count() > 64
            || self.name.chars().any(char::is_control)
            || !(1..=365).contains(&self.expires_in_days)
            || self.scopes.is_empty()
            || self.scopes.len() > 128
        {
            return Err(AppError::Validation(
                "Token 名称、有效期或 Scope 无效".into(),
            ));
        }
        let implemented = crate::modules::api_catalog::endpoints()
            .into_iter()
            .flat_map(|endpoint| endpoint.required_permissions.iter().copied())
            .collect::<std::collections::HashSet<_>>();
        if self
            .scopes
            .iter()
            .any(|scope| scope == "*" || !implemented.contains(scope.as_str()))
        {
            return Err(AppError::Validation(
                "Scope 必须是已实现的明确操作码".into(),
            ));
        }
        self.scopes.sort();
        let count = self.scopes.len();
        self.scopes.dedup();
        if self.scopes.len() != count {
            return Err(AppError::Validation("Scope 不能重复".into()));
        }
        let policy = self.policy().normalize()?;
        self.allowed_ips = policy.allowed_ips;
        Ok(self)
    }
    pub fn policy(&self) -> Policy {
        Policy {
            allowed_ips: self.allowed_ips.clone(),
            minute_limit: self.minute_limit,
            daily_quota: self.daily_quota,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
