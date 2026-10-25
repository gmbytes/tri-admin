use crate::common::pagination::PageInput;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Login,
    #[default]
    Operation,
    Error,
}
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Operation => "operation",
            Self::Error => "error",
        }
    }
    pub fn collection(self) -> &'static str {
        if self == Self::Error {
            "request_errors"
        } else {
            "audit_events"
        }
    }
}
fn page() -> u64 {
    1
}
fn size() -> u64 {
    20
}
fn limit() -> u32 {
    1000
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogInput {
    #[serde(default)]
    pub kind: Kind,
    #[serde(default = "page")]
    pub page: u64,
    #[serde(default = "size")]
    pub page_size: u64,
    pub sort: Option<String>,
    pub action: Option<String>,
    pub username: Option<String>,
    pub actor_id: Option<String>,
    pub request_id: Option<String>,
    pub outcome: Option<String>,
    pub status: Option<u16>,
    pub from: Option<String>,
    pub to: Option<String>,
}
impl LogInput {
    pub fn page(&self) -> PageInput {
        PageInput {
            page: self.page,
            page_size: self.page_size,
            sort: self.sort.clone(),
            search: None,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupInput {
    pub request_id: String,
    pub kind: Kind,
    pub before: String,
    #[serde(default = "limit")]
    pub limit: u32,
}
