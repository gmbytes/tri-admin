use super::{dto::Integration, validate};
use crate::{
    common::{error::AppError, vault::SealedBox},
    config::Config,
};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub fn stable_id(label: &str) -> ObjectId {
    let hash = Sha256::digest(format!("tri-admin:settings:{label}"));
    let mut bytes = [0u8; 12];
    bytes.copy_from_slice(&hash[..12]);
    ObjectId::from_bytes(bytes)
}
pub fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::Internal)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub code: String,
    pub description: String,
    pub kind: super::dto::Kind,
    pub configuration_json: String,
    pub secret_box: Option<SealedBox>,
    pub enabled: bool,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub last_check: Option<super::check_model::CheckRecord>,
}
impl Profile {
    pub fn configuration(&self) -> Result<Integration, AppError> {
        let value: Integration =
            serde_json::from_str(&self.configuration_json).map_err(|_| AppError::NotReady)?;
        validate::public(&value).map_err(|_| AppError::NotReady)?;
        if value.kind() != self.kind {
            return Err(AppError::NotReady);
        }
        Ok(value)
    }
    pub fn view(self, config: &Config) -> Result<ProfileView, AppError> {
        let configuration = self.configuration()?;
        let last_check = self
            .last_check
            .as_ref()
            .map(|check| {
                check.view(
                    check.profile_version == self.version
                        && check.fingerprint == super::check_model::fingerprint(&self, config)?,
                )
            })
            .transpose()?;
        let credentials = CredentialState {
            configured: self.secret_box.is_some(),
            key_id: self.secret_box.as_ref().map(|value| value.key_id.clone()),
            key_available: self
                .secret_box
                .as_ref()
                .is_some_and(|value| config.settings_key(&value.key_id).is_some()),
        };
        Ok(ProfileView {
            id: self.id.to_hex(),
            name: self.name,
            code: self.code,
            description: self.description,
            enabled: self.enabled,
            configuration,
            credentials,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
            last_check,
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialState {
    pub configured: bool,
    pub key_id: Option<String>,
    pub key_available: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub id: String,
    pub name: String,
    pub code: String,
    pub description: String,
    pub enabled: bool,
    pub configuration: Integration,
    pub credentials: CredentialState,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_check: Option<super::check_model::CheckView>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSettings {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub display_name: String,
    pub login_limit_per_ip_override: Option<i64>,
    pub storage_profile_id: Option<ObjectId>,
    pub mail_profile_id: Option<ObjectId>,
    pub version: i64,
    pub updated_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    pub display_name: String,
    pub login_limit_per_ip_override: Option<i64>,
    pub effective_login_limit_per_ip: i64,
    pub storage_profile_id: Option<String>,
    pub mail_profile_id: Option<String>,
    pub version: i64,
    pub updated_at: String,
}
impl ServiceSettings {
    pub fn view(self, config: &Config) -> Result<ServiceView, AppError> {
        Ok(ServiceView {
            display_name: self.display_name,
            login_limit_per_ip_override: self.login_limit_per_ip_override,
            effective_login_limit_per_ip: self
                .login_limit_per_ip_override
                .unwrap_or(config.login_limit_per_ip),
            storage_profile_id: self.storage_profile_id.map(ObjectId::to_hex),
            mail_profile_id: self.mail_profile_id.map(ObjectId::to_hex),
            version: self.version,
            updated_at: time(self.updated_at)?,
        })
    }
}
