use crate::common::error::AppError;
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StampView {
    pub id: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl Stamp {
    fn view(self) -> Result<StampView, AppError> {
        fn time(value: DateTime) -> Result<String, AppError> {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
                .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                .ok_or(AppError::Internal)
        }
        Ok(StampView {
            id: self.id.to_hex(),
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DictionaryRecord {
    #[serde(flatten)]
    pub stamp: Stamp,
    pub name: String,
    pub key: String,
    pub description: String,
    pub enabled: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DictionaryView {
    #[serde(flatten)]
    pub stamp: StampView,
    pub name: String,
    pub key: String,
    pub description: String,
    pub enabled: bool,
}
impl DictionaryRecord {
    pub fn view(self) -> Result<DictionaryView, AppError> {
        Ok(DictionaryView {
            stamp: self.stamp.view()?,
            name: self.name,
            key: self.key,
            description: self.description,
            enabled: self.enabled,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemRecord {
    #[serde(flatten)]
    pub stamp: Stamp,
    pub dictionary_id: ObjectId,
    pub label: String,
    pub value: String,
    pub sort: i32,
    pub enabled: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    #[serde(flatten)]
    pub stamp: StampView,
    pub dictionary_id: String,
    pub label: String,
    pub value: String,
    pub sort: i32,
    pub enabled: bool,
}
impl ItemRecord {
    pub fn view(self) -> Result<ItemView, AppError> {
        Ok(ItemView {
            stamp: self.stamp.view()?,
            dictionary_id: self.dictionary_id.to_hex(),
            label: self.label,
            value: self.value,
            sort: self.sort,
            enabled: self.enabled,
        })
    }
}
