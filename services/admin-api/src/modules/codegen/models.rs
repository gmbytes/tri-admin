use crate::common::error::AppError;
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
use tri_resource_runtime::schema::Schema;
fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_millis(value.timestamp_millis())
        .map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::NotReady)
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionMeta {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub schema: Schema,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Deserialize, Serialize)]
pub struct DefinitionRecord {
    #[serde(flatten)]
    pub meta: DefinitionMeta,
    pub fingerprint: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionView {
    pub id: String,
    pub owner_id: String,
    pub request_id: String,
    pub schema: Schema,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}
impl DefinitionMeta {
    pub fn view(self) -> Result<DefinitionView, AppError> {
        self.schema.validate().map_err(|_| AppError::NotReady)?;
        Ok(DefinitionView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            request_id: self.request_id,
            schema: self.schema,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            deleted_at: self.deleted_at.map(time).transpose()?,
        })
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationMeta {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub definition_id: ObjectId,
    pub definition_version: i64,
    pub template_version: u32,
    pub schema: Schema,
    pub schema_checksum: String,
    pub sha256: String,
    pub size: u64,
    pub files: Vec<FileMeta>,
    pub created_at: DateTime,
}
#[derive(Deserialize, Serialize)]
pub struct GenerationRecord {
    #[serde(flatten)]
    pub meta: GenerationMeta,
    pub fingerprint: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationView {
    pub id: String,
    pub owner_id: String,
    pub request_id: String,
    pub definition_id: String,
    pub definition_version: i64,
    pub template_version: u32,
    pub schema: Schema,
    pub schema_checksum: String,
    pub sha256: String,
    pub size: u64,
    pub files: Vec<FileMeta>,
    pub created_at: String,
}
impl GenerationMeta {
    pub fn view(self) -> Result<GenerationView, AppError> {
        self.schema.validate().map_err(|_| AppError::NotReady)?;
        Ok(GenerationView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            request_id: self.request_id,
            definition_id: self.definition_id.to_hex(),
            definition_version: self.definition_version,
            template_version: self.template_version,
            schema: self.schema,
            schema_checksum: self.schema_checksum,
            sha256: self.sha256,
            size: self.size,
            files: self.files,
            created_at: time(self.created_at)?,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPackage {
    pub archive_bytes: mongodb::bson::Binary,
}
#[derive(Serialize)]
pub struct SourceView {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub sha256: String,
}
