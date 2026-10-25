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
pub struct InspectInput {
    pub resource: String,
    pub format: Format,
    pub content_base64: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub resource: String,
    pub format: Format,
    pub sha256: String,
    pub headers: Vec<String>,
    pub row_count: usize,
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
    pub values_json: Option<String>,
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
    #[serde(flatten)]
    pub counts: ImportCounts,
    pub payload_state: String,
    pub redacted_at: Option<DateTime>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCounts {
    pub row_count: i64,
    pub ready_count: i64,
    pub applied_count: i64,
    pub error_count: i64,
    pub expired_count: i64,
}
impl ImportCounts {
    pub fn from_rows(rows: &[ImportRow]) -> Result<Self, crate::common::error::AppError> {
        let mut counts = Self {
            row_count: rows.len() as i64,
            ready_count: 0,
            applied_count: 0,
            error_count: 0,
            expired_count: 0,
        };
        for row in rows {
            match row.status.as_str() {
                "ready" => counts.ready_count += 1,
                "applied" => counts.applied_count += 1,
                "expired" => {
                    counts.error_count += 1;
                    counts.expired_count += 1
                }
                "invalid" | "duplicate" | "conflict" => counts.error_count += 1,
                _ => return Err(crate::common::error::AppError::Internal),
            }
        }
        Ok(counts)
    }
    fn validate(&self) -> Result<(), crate::common::error::AppError> {
        if self.row_count < 0
            || self.row_count > 2000
            || self.ready_count < 0
            || self.applied_count < 0
            || self.error_count < 0
            || self.expired_count < 0
            || self.ready_count > self.row_count
            || self.applied_count > self.row_count
            || self.error_count > self.row_count
            || self.expired_count > self.error_count
            || self.ready_count + self.applied_count + self.error_count != self.row_count
        {
            return Err(crate::common::error::AppError::Internal);
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportMeta {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub request_id: String,
    pub resource: String,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    #[serde(flatten)]
    pub counts: ImportCounts,
    pub payload_state: String,
    pub redacted_at: Option<DateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub id: String,
    pub request_id: String,
    pub resource: String,
    pub created_at: String,
    pub expires_at: String,
    #[serde(flatten)]
    pub counts: ImportCounts,
    pub status: String,
    pub payload_state: String,
    pub redacted_at: Option<String>,
}
impl ImportMeta {
    pub fn summary(self, now: DateTime) -> Result<ImportSummary, crate::common::error::AppError> {
        self.counts.validate()?;
        if !matches!(self.payload_state.as_str(), "full" | "redacted") {
            return Err(crate::common::error::AppError::Internal);
        }
        let mut counts = self.counts;
        if self.expires_at <= now {
            counts.error_count += counts.ready_count;
            counts.expired_count += counts.ready_count;
            counts.ready_count = 0;
        }
        let status = if counts.ready_count > 0 {
            "pending"
        } else if counts.expired_count > 0 && counts.applied_count == 0 {
            "expired"
        } else if counts.error_count > 0 {
            if counts.applied_count > 0 {
                "partial"
            } else {
                "failed"
            }
        } else {
            "completed"
        };
        let payload_state = if self.payload_state == "redacted" {
            "redacted"
        } else if self.expires_at <= now {
            "expired"
        } else {
            "full"
        };
        Ok(ImportSummary {
            id: self.id.to_hex(),
            request_id: self.request_id,
            resource: self.resource,
            created_at: public_time(self.created_at)?,
            expires_at: public_time(self.expires_at)?,
            counts,
            status: status.into(),
            payload_state: payload_state.into(),
            redacted_at: self.redacted_at.map(public_time).transpose()?,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportView {
    #[serde(flatten)]
    pub summary: ImportSummary,
    pub rows: Vec<RowView>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    pub index: usize,
    pub key: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub target_id: Option<String>,
    pub values: Option<serde_json::Value>,
}
impl ImportRecord {
    pub fn view(self) -> Result<ImportView, crate::common::error::AppError> {
        let now = DateTime::now();
        let summary = ImportMeta {
            id: self.id,
            request_id: self.request_id,
            resource: self.resource,
            created_at: self.created_at,
            expires_at: self.expires_at,
            counts: self.counts,
            payload_state: self.payload_state,
            redacted_at: self.redacted_at,
        }
        .summary(now)?;
        let available = summary.payload_state == "full";
        let expired = self.expires_at <= now;
        let rows = self
            .rows
            .into_iter()
            .map(|row| {
                Ok::<_, crate::common::error::AppError>(RowView {
                    index: row.index,
                    key: if available { Some(row.key) } else { None },
                    status: if expired && row.status == "ready" {
                        "expired".into()
                    } else {
                        row.status.clone()
                    },
                    error: if expired && row.status == "ready" {
                        Some("PREVIEW_EXPIRED".into())
                    } else {
                        row.error
                    },
                    target_id: row.target_id,
                    values: if available {
                        Some(
                            serde_json::from_str(
                                row.values_json
                                    .as_deref()
                                    .ok_or(crate::common::error::AppError::Internal)?,
                            )
                            .map_err(|_| crate::common::error::AppError::Internal)?,
                        )
                    } else {
                        None
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ImportView { summary, rows })
    }
}

pub(crate) fn public_time(value: DateTime) -> Result<String, crate::common::error::AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(crate::common::error::AppError::Internal)
}
