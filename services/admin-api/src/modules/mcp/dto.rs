pub use crate::modules::codegen::dto::{request_id, version};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TransportKind {
    Stdio,
    Http,
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Transport {
    Stdio {
        #[serde(rename = "capabilityId")]
        capability_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Http {
        profile_id: String,
        profile_version: i64,
        allowed_tools: Vec<String>,
    },
}
impl Transport {
    pub fn kind(&self) -> TransportKind {
        match self {
            Self::Stdio { .. } => TransportKind::Stdio,
            Self::Http { .. } => TransportKind::Http,
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Create {
    pub request_id: String,
    pub name: String,
    pub transport: Transport,
    #[serde(default)]
    pub enabled: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Patch {
    pub version: i64,
    pub name: String,
    pub transport: Transport,
    pub enabled: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionKind {
    Start,
    Stop,
    Restart,
}
impl ActionKind {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionInput {
    pub request_id: String,
    pub version: i64,
    pub kind: ActionKind,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Call {
    pub request_id: String,
    pub version: i64,
    pub name: String,
    pub arguments: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    pub version: i64,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelInput {
    pub request_id: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrphanConfirm {
    pub request_id: String,
    pub version: i64,
    pub confirmed: bool,
    pub reason: String,
}
