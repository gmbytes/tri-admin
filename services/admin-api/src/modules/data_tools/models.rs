use super::formats::Format;
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewInput {
    pub request_id: String,
    pub resource: String,
    pub format: Format,
    pub content_base64: String,
    #[serde(default)]
    pub mapping: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportInput {
    pub resource: String,
    pub format: Format,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub search: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub rows: Vec<usize>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRow {
    pub index: usize,
    pub key: String,
    pub status: String,
    pub error: Option<String>,
    pub target_id: Option<String>,
    pub values_json: String,
    pub parent_id: Option<ObjectId>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub actor_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub resource: String,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    pub rows: Vec<ImportRow>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportView {
    pub id: String,
    pub request_id: String,
    pub resource: String,
    pub expires_at: String,
    pub rows: Vec<RowView>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    pub index: usize,
    pub key: String,
    pub status: String,
    pub error: Option<String>,
    pub target_id: Option<String>,
    pub values: serde_json::Value,
}
impl ImportRecord {
    pub fn view(self) -> Result<ImportView, crate::common::error::AppError> {
        let rows = self
            .rows
            .into_iter()
            .map(|row| {
                Ok::<_, crate::common::error::AppError>(RowView {
                    index: row.index,
                    key: row.key,
                    status: row.status,
                    error: row.error,
                    target_id: row.target_id,
                    values: serde_json::from_str(&row.values_json)
                        .map_err(|_| crate::common::error::AppError::Internal)?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ImportView {
            id: self.id.to_hex(),
            request_id: self.request_id,
            resource: self.resource,
            expires_at: public_time(self.expires_at)?,
            rows,
        })
    }
}

pub(crate) fn public_time(value: DateTime) -> Result<String, crate::common::error::AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(crate::common::error::AppError::Internal)
}
