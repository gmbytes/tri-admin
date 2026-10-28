use super::{
    dto::{Integration, Kind},
    validate,
};
use crate::common::{error::AppError, vault::SealedBox};
use mongodb::bson::oid::ObjectId;
use serde::Deserialize;
use sha2::{Digest, Sha256};
pub fn stable_id(label: &str) -> ObjectId {
    let hash = Sha256::digest(format!("tri-admin:settings:{label}"));
    let mut bytes = [0; 12];
    bytes.copy_from_slice(&hash[..12]);
    ObjectId::from_bytes(bytes)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub code: String,
    pub kind: Kind,
    pub configuration_json: String,
    pub secret_box: Option<SealedBox>,
    pub enabled: bool,
    pub version: i64,
}
impl Profile {
    pub fn configuration(&self) -> Result<Integration, AppError> {
        if !self.enabled {
            return Err(AppError::NotReady);
        }
        let value: Integration =
            serde_json::from_str(&self.configuration_json).map_err(|_| AppError::NotReady)?;
        validate::public(&value).map_err(|_| AppError::NotReady)?;
        if value.kind() != self.kind {
            return Err(AppError::NotReady);
        }
        Ok(value)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSettings {
    pub storage_profile_id: Option<ObjectId>,
}
