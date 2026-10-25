use super::models::{Bundle, Dictionary, Item, Menu, Parameter};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Strategy {
    #[default]
    CreateOnly,
    Merge,
}
impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CreateOnly => "createOnly",
            Self::Merge => "merge",
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewInput {
    pub bundle: Bundle,
    #[serde(default)]
    pub strategy: Strategy,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub create: u64,
    pub update: u64,
    pub unchanged: u64,
    pub conflict: u64,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCounts {
    pub menus: Changes,
    pub dictionaries: Changes,
    pub dictionary_items: Changes,
    pub parameters: Changes,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub resource: String,
    pub key: String,
    pub action: String,
    pub fields: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub counts: ImportCounts,
    pub changes: Vec<Change>,
    pub details_truncated: bool,
    pub version_action: String,
    pub can_apply: bool,
    pub missing_write_permissions: Vec<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewView {
    pub id: String,
    pub checksum: String,
    pub expires_at: String,
    pub strategy: Strategy,
    #[serde(flatten)]
    pub plan: Plan,
}
#[derive(Serialize)]
pub struct MenuState {
    pub id: ObjectId,
    pub version: i64,
    pub config: Menu,
}
#[derive(Serialize)]
pub struct ItemState {
    pub id: ObjectId,
    pub version: i64,
    pub config: Item,
}
#[derive(Serialize)]
pub struct DictionaryState {
    pub id: ObjectId,
    pub version: i64,
    pub fence: i64,
    pub config: Dictionary,
    pub items: Vec<ItemState>,
}
#[derive(Serialize)]
pub struct ParameterState {
    pub id: ObjectId,
    pub version: i64,
    pub config: Parameter,
}
#[derive(Serialize)]
pub struct VersionState {
    pub id: ObjectId,
    pub version: i64,
    pub checksum: String,
    pub deleted: bool,
    pub payload_valid: bool,
}
#[derive(Default, Serialize)]
pub struct TargetState {
    pub menus: Vec<MenuState>,
    pub dictionaries: Vec<DictionaryState>,
    pub parameters: Vec<ParameterState>,
    pub version: Option<VersionState>,
}
