use crate::{common::error::AppError, modules::open_access::limits::Policy};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::Internal)
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenMeta {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub owner_id: ObjectId,
    pub request_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub allowed_ips: Vec<String>,
    pub minute_limit: u32,
    pub daily_quota: u32,
    pub created_at: DateTime,
    pub expires_at: DateTime,
    pub revoked_at: Option<DateTime>,
    pub last_used_at: Option<DateTime>,
}
impl TokenMeta {
    pub fn policy(&self) -> Policy {
        Policy {
            allowed_ips: self.allowed_ips.clone(),
            minute_limit: self.minute_limit,
            daily_quota: self.daily_quota,
        }
    }
    pub fn view(self) -> Result<TokenView, AppError> {
        let status = if self.revoked_at.is_some() {
            "revoked"
        } else if self.expires_at <= DateTime::now() {
            "expired"
        } else {
            "active"
        };
        Ok(TokenView {
            id: self.id.to_hex(),
            request_id: self.request_id,
            name: self.name,
            scopes: self.scopes,
            allowed_ips: self.allowed_ips,
            minute_limit: self.minute_limit,
            daily_quota: self.daily_quota,
            status,
            created_at: time(self.created_at)?,
            expires_at: time(self.expires_at)?,
            revoked_at: self.revoked_at.map(time).transpose()?,
            last_used_at: self.last_used_at.map(time).transpose()?,
        })
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRecord {
    #[serde(flatten)]
    pub meta: TokenMeta,
    pub secret_hash: String,
    pub fingerprint: String,
    pub usage_fence: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenView {
    pub id: String,
    pub request_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub allowed_ips: Vec<String>,
    pub minute_limit: u32,
    pub daily_quota: u32,
    pub status: &'static str,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    pub last_used_at: Option<String>,
}
#[derive(Serialize)]
pub struct IssueView {
    pub token: TokenView,
    pub secret: Option<String>,
}
pub fn projection() -> mongodb::bson::Document {
    mongodb::bson::doc! {"secretHash":0,"fingerprint":0,"usageFence":0}
}
