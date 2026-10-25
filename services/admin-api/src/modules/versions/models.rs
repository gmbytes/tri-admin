use crate::{common::error::AppError, modules::parameters::dto::ValueType};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const MAX_PACKAGE_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bundle {
    pub format: String,
    pub schema_version: u32,
    pub release: Release,
    pub resources: Resources,
    pub required_apis: Vec<ApiDependency>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    pub name: String,
    pub code: String,
    pub description: String,
    pub captured_at: String,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resources {
    pub menus: Vec<Menu>,
    pub dictionaries: Vec<Dictionary>,
    pub parameters: Vec<Parameter>,
}
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Menu {
    pub label: String,
    pub path: String,
    pub parent_path: Option<String>,
    pub view_key: Option<String>,
    pub icon: Option<String>,
    pub permission: Option<String>,
    pub sort: i32,
    pub enabled: bool,
}
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dictionary {
    pub name: String,
    pub key: String,
    pub description: String,
    pub enabled: bool,
    pub items: Vec<Item>,
}
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item {
    pub label: String,
    pub value: String,
    pub sort: i32,
    pub enabled: bool,
}
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Parameter {
    pub name: String,
    pub key: String,
    pub group: String,
    pub description: String,
    pub value_type: ValueType,
    pub value: Value,
}
#[derive(Clone, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiDependency {
    pub operation_id: String,
    pub method: String,
    pub path: String,
    pub required_permissions: Vec<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub menus: i64,
    pub ancestors: i64,
    pub dictionaries: i64,
    pub dictionary_items: i64,
    pub parameters: i64,
    pub apis: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub code: String,
    pub description: String,
    pub checksum: String,
    pub byte_length: i64,
    pub counts: Counts,
    pub read_permissions: Vec<String>,
    pub created_by_id: ObjectId,
    pub created_by_name: String,
    pub origin: String,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
pub struct StoredPackage {
    pub meta: MetaRecord,
    pub package_json: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageRecord {
    pub package_json: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Creator {
    pub id: String,
    pub name: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaView {
    pub id: String,
    pub name: String,
    pub code: String,
    pub description: String,
    pub checksum: String,
    pub byte_length: i64,
    pub counts: Counts,
    pub read_permissions: Vec<String>,
    pub created_by: Creator,
    pub origin: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
pub fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::Internal)
}
impl MetaRecord {
    pub fn view(self) -> Result<MetaView, AppError> {
        Ok(MetaView {
            id: self.id.to_hex(),
            name: self.name,
            code: self.code,
            description: self.description,
            checksum: self.checksum,
            byte_length: self.byte_length,
            counts: self.counts,
            read_permissions: self.read_permissions,
            created_by: Creator {
                id: self.created_by_id.to_hex(),
                name: self.created_by_name,
            },
            origin: self.origin,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
