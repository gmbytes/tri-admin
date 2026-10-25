use super::dto::ValueType;
use crate::common::error::AppError;
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub key: String,
    pub group: String,
    pub description: String,
    pub value_type: ValueType,
    pub value_json: String,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterView {
    pub id: String,
    pub name: String,
    pub key: String,
    pub group: String,
    pub description: String,
    pub value_type: ValueType,
    pub value: Value,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl ParameterRecord {
    pub fn view(self) -> Result<ParameterView, AppError> {
        fn time(value: DateTime) -> Result<String, AppError> {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
                .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                .ok_or(AppError::Internal)
        }
        Ok(ParameterView {
            id: self.id.to_hex(),
            name: self.name,
            key: self.key,
            group: self.group,
            description: self.description,
            value_type: self.value_type,
            value: serde_json::from_str(&self.value_json).map_err(|_| AppError::Internal)?,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
