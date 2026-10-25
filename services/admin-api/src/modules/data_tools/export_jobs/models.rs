use super::super::{
    formats::Format,
    models::{TemplateOrigin, public_time},
    query::{DataFilter, DataSort},
    templates::Column,
};
use crate::{
    common::{error::AppError, model::DataScope},
    modules::files::storage::BlobSpec,
};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};

fn default_rows() -> u32 {
    100_000
}
fn first() -> u64 {
    1
}
fn page_size() -> u64 {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListInput {
    #[serde(default = "first")]
    pub page: u64,
    #[serde(default = "page_size")]
    pub page_size: u64,
    pub resource: Option<String>,
    pub state: Option<JobState>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateInput {
    pub request_id: String,
    pub format: Format,
    #[serde(default = "default_rows")]
    pub max_rows: u32,
    pub storage_profile_id: Option<String>,
    pub source: Source,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Source {
    Resource {
        resource: String,
        columns: Vec<Column>,
        #[serde(default)]
        search: String,
        #[serde(default)]
        filters: Vec<DataFilter>,
        sort: Option<DataSort>,
    },
    Template {
        id: String,
        version: i64,
    },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportSpec {
    pub resource: String,
    pub columns: Vec<Column>,
    pub search: String,
    pub filters: Vec<DataFilter>,
    pub sort: Option<DataSort>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeSnapshot {
    pub scope: DataScope,
    pub scope_role_ids: Vec<ObjectId>,
}
#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Running,
    Publishing,
    Reconciling,
    Unknown,
    Completed,
    Failed,
    Cancelled,
    Expired,
}
pub const ACTIVE: &[&str] = &["queued", "running", "publishing", "reconciling", "unknown"];
impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Publishing => "publishing",
            Self::Reconciling => "reconciling",
            Self::Unknown => "unknown",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub blob_id: ObjectId,
    pub size: i64,
    pub sha256: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobMeta {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub request_id: String,
    pub resource: String,
    pub format: Format,
    pub max_rows: u32,
    pub template: Option<TemplateOrigin>,
    pub state: JobState,
    pub row_count: u32,
    pub output: Option<Output>,
    pub last_error: Option<String>,
    pub cleanup_pending: bool,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub started_at: Option<DateTime>,
    pub completed_at: Option<DateTime>,
    pub expires_at: Option<DateTime>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    #[serde(flatten)]
    pub meta: JobMeta,
    pub actor_id: ObjectId,
    pub fingerprint: String,
    pub spec: ExportSpec,
    pub creation_scope: ScopeSnapshot,
    pub storage_profile_id: ObjectId,
    pub storage_retained: bool,
    pub generation: i64,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<DateTime>,
    pub candidate_id: Option<ObjectId>,
    pub next_run_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub name: String,
    pub mime_type: &'static str,
    pub size: u64,
    pub sha256: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: String,
    pub request_id: String,
    pub resource: String,
    pub format: Format,
    pub max_rows: u32,
    pub template: Option<TemplateOrigin>,
    pub state: JobState,
    pub row_count: u32,
    pub file: Option<FileView>,
    pub last_error: Option<String>,
    pub cleanup_pending: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub expires_at: Option<String>,
}
impl JobMeta {
    pub fn view(self, now: DateTime) -> Result<JobView, AppError> {
        let state = if self.state == JobState::Completed
            && self.expires_at.is_some_and(|time| time <= now)
        {
            JobState::Expired
        } else {
            self.state
        };
        let cleanup_pending =
            self.cleanup_pending || (state == JobState::Expired && self.output.is_some());
        let file = if state == JobState::Completed {
            if self.expires_at.is_none() {
                return Err(AppError::NotReady);
            }
            let value = self.output.ok_or(AppError::NotReady)?;
            let size = u64::try_from(value.size).map_err(|_| AppError::NotReady)?;
            let spec = BlobSpec::new(size, &value.sha256).map_err(|_| AppError::NotReady)?;
            let (extension, mime_type) = match self.format {
                Format::Csv => ("csv", "text/csv;charset=utf-8"),
                Format::Xlsx => (
                    "xlsx",
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                ),
            };
            Some(FileView {
                name: format!(
                    "tri-admin-{}-{}.{}",
                    self.resource,
                    self.id.to_hex(),
                    extension
                ),
                mime_type,
                size: spec.size(),
                sha256: spec.sha256().to_owned(),
            })
        } else {
            None
        };
        Ok(JobView {
            id: self.id.to_hex(),
            request_id: self.request_id,
            resource: self.resource,
            format: self.format,
            max_rows: self.max_rows,
            template: self.template,
            state,
            row_count: self.row_count,
            file,
            last_error: self.last_error,
            cleanup_pending,
            version: self.version,
            created_at: public_time(self.created_at)?,
            updated_at: public_time(self.updated_at)?,
            started_at: self.started_at.map(public_time).transpose()?,
            completed_at: self.completed_at.map(public_time).transpose()?,
            expires_at: self.expires_at.map(public_time).transpose()?,
        })
    }
}
