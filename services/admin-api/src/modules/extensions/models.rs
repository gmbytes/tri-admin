use crate::common::error::AppError;
use mongodb::bson::{Binary, DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
use tri_extension_package::package::{self, PackageInfo};
pub fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_millis(value.timestamp_millis())
        .map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::NotReady)
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub name: String,
    pub kind: package::Kind,
    pub enabled: bool,
    pub version: i64,
    pub revision: i64,
    pub revision_id: ObjectId,
    pub package: PackageInfo,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionView {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub kind: package::Kind,
    pub enabled: bool,
    pub version: i64,
    pub revision: i64,
    pub revision_id: String,
    pub package: PackageInfo,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}
impl Extension {
    pub fn view(self) -> Result<ExtensionView, AppError> {
        if !package::valid_name(&self.name)
            || self.name != self.package.name
            || self.kind != self.package.kind
            || self.version < 1
            || self.revision < 1
        {
            return Err(AppError::NotReady);
        }
        Ok(ExtensionView {
            id: self.id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            name: self.name,
            kind: self.kind,
            enabled: self.enabled,
            version: self.version,
            revision: self.revision,
            revision_id: self.revision_id.to_hex(),
            package: self.package,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            deleted_at: self.deleted_at.map(time).transpose()?,
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionMeta {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub extension_id: ObjectId,
    pub owner_id: ObjectId,
    pub actor_id: ObjectId,
    pub request_id: String,
    pub number: i64,
    pub package: PackageInfo,
    pub created_at: DateTime,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    #[serde(flatten)]
    pub meta: RevisionMeta,
    pub archive_bytes: Binary,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionView {
    pub id: String,
    pub extension_id: String,
    pub owner_id: String,
    pub actor_id: String,
    pub request_id: String,
    pub number: i64,
    pub package: PackageInfo,
    pub created_at: String,
}
impl RevisionMeta {
    pub fn view(self) -> Result<RevisionView, AppError> {
        Ok(RevisionView {
            id: self.id.to_hex(),
            extension_id: self.extension_id.to_hex(),
            owner_id: self.owner_id.to_hex(),
            actor_id: self.actor_id.to_hex(),
            request_id: self.request_id,
            number: self.number,
            package: self.package,
            created_at: time(self.created_at)?,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub actor_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub result: Extension,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Constraint {
    pub version: i64,
    pub content: String,
    pub updated_at: DateTime,
    pub actor_id: Option<ObjectId>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConstraintView {
    pub version: i64,
    pub content: String,
    pub updated_at: String,
    pub actor_id: Option<String>,
}
impl Constraint {
    pub fn view(self) -> Result<ConstraintView, AppError> {
        if self.version < 1 || self.content.len() > 64 * 1024 {
            return Err(AppError::NotReady);
        }
        Ok(ConstraintView {
            version: self.version,
            content: self.content,
            updated_at: time(self.updated_at)?,
            actor_id: self.actor_id.map(|id| id.to_hex()),
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewView {
    pub path: String,
    pub content: String,
    pub sha256: String,
    pub size: u64,
}
