use super::dto::UserStatus;
use crate::{common::error::AppError, modules::open_access::scopes::Scope};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
pub fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::NotReady)
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationMeta {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub created_by: ObjectId,
    pub request_id: String,
    pub name: String,
    pub code: String,
    pub enabled: bool,
    pub allowed_scopes: Vec<Scope>,
    pub allowed_ips: Vec<String>,
    pub minute_limit: u32,
    pub daily_quota: u32,
    pub credential_version: i64,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationRecord {
    #[serde(flatten)]
    pub meta: ApplicationMeta,
    pub secret_hash: String,
    pub fingerprint: String,
    pub write_fence: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationView {
    pub id: String,
    pub request_id: String,
    pub name: String,
    pub code: String,
    pub enabled: bool,
    pub allowed_scopes: Vec<Scope>,
    pub allowed_ips: Vec<String>,
    pub minute_limit: u32,
    pub daily_quota: u32,
    pub credential_version: i64,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}
impl ApplicationMeta {
    pub fn view(self) -> Result<ApplicationView, AppError> {
        Ok(ApplicationView {
            id: self.id.to_hex(),
            request_id: self.request_id,
            name: self.name,
            code: self.code,
            enabled: self.enabled,
            allowed_scopes: self.allowed_scopes,
            allowed_ips: self.allowed_ips,
            minute_limit: self.minute_limit,
            daily_quota: self.daily_quota,
            credential_version: self.credential_version,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            deleted_at: self.deleted_at.map(time).transpose()?,
        })
    }
}
#[derive(Serialize)]
pub struct IssuedApplication {
    pub application: ApplicationView,
    pub secret: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub application_id: ObjectId,
    pub actor_id: ObjectId,
    pub request_id: String,
    pub fingerprint: String,
    pub version: i64,
    pub credential_version: i64,
    pub created_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionView {
    pub application_id: String,
    pub request_id: String,
    pub kind: &'static str,
    pub version: i64,
    pub credential_version: i64,
    pub created_at: String,
}
impl ActionRecord {
    pub fn view(self) -> Result<ActionView, AppError> {
        Ok(ActionView {
            application_id: self.application_id.to_hex(),
            request_id: self.request_id,
            kind: "rotateSecret",
            version: self.version,
            credential_version: self.credential_version,
            created_at: time(self.created_at)?,
        })
    }
}
#[derive(Serialize)]
pub struct RotatedApplication {
    pub application: ApplicationView,
    pub secret: Option<String>,
    pub receipt: ActionView,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMeta {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub application_id: ObjectId,
    pub username: String,
    pub username_key: String,
    pub display_name: String,
    pub scopes: Vec<Scope>,
    pub status: UserStatus,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub deleted_at: Option<DateTime>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRecord {
    #[serde(flatten)]
    pub meta: UserMeta,
    pub password_hash: String,
    pub auth_version: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserView {
    pub id: String,
    pub application_id: String,
    pub username: String,
    pub display_name: String,
    pub scopes: Vec<Scope>,
    pub status: UserStatus,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl UserMeta {
    pub fn view(self) -> Result<UserView, AppError> {
        Ok(UserView {
            id: self.id.to_hex(),
            application_id: self.application_id.to_hex(),
            username: self.username,
            display_name: self.display_name,
            scopes: self.scopes,
            status: self.status,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
pub fn application_projection() -> mongodb::bson::Document {
    mongodb::bson::doc! {"secretHash":0,"fingerprint":0,"writeFence":0}
}
pub fn user_projection() -> mongodb::bson::Document {
    mongodb::bson::doc! {"passwordHash":0,"authVersion":0}
}
