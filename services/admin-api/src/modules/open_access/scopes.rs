use crate::common::error::AppError;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
pub enum Scope {
    #[serde(rename = "open.profile.read")]
    ProfileRead,
    #[serde(rename = "open.profile.write")]
    ProfileWrite,
    #[serde(rename = "open.users.read")]
    UsersRead,
    #[serde(rename = "open.users.write")]
    UsersWrite,
}
impl Scope {
    pub fn is_profile(self) -> bool {
        matches!(self, Self::ProfileRead | Self::ProfileWrite)
    }
    pub fn admin_permission(self) -> &'static str {
        match self {
            Self::ProfileRead | Self::UsersRead => "applicationUsers.read",
            Self::ProfileWrite | Self::UsersWrite => "applicationUsers.write",
        }
    }
}
pub fn normalize(mut scopes: Vec<Scope>, profile_only: bool) -> Result<Vec<Scope>, AppError> {
    if scopes.is_empty()
        || scopes.len() > 4
        || profile_only && scopes.iter().any(|s| !s.is_profile())
    {
        return Err(AppError::Validation("开放 Scope 无效".into()));
    }
    let size = scopes.len();
    scopes.sort();
    scopes.dedup();
    if scopes.len() != size {
        return Err(AppError::Validation("开放 Scope 不能重复".into()));
    }
    Ok(scopes)
}
