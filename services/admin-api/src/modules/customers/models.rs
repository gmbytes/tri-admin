use super::dto::CustomerStatus;
use crate::common::error::AppError;
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomerRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub code: String,
    pub name: String,
    pub contact_name: String,
    pub phone: String,
    pub email: String,
    pub note: String,
    pub status: CustomerStatus,
    pub owner_id: ObjectId,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomerView {
    pub id: String,
    pub code: String,
    pub name: String,
    pub contact_name: String,
    pub phone: String,
    pub email: String,
    pub note: String,
    pub status: CustomerStatus,
    pub owner_id: String,
    pub owner: Option<OwnerView>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub username: String,
    pub display_name: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerView {
    pub id: String,
    pub username: String,
    pub display_name: String,
}
impl OwnerRecord {
    pub fn view(self) -> OwnerView {
        OwnerView {
            id: self.id.to_hex(),
            username: self.username,
            display_name: self.display_name,
        }
    }
}
impl CustomerRecord {
    pub fn view(self) -> Result<CustomerView, AppError> {
        fn time(value: DateTime) -> Result<String, AppError> {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
                .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                .ok_or(AppError::Internal)
        }
        Ok(CustomerView {
            id: self.id.to_hex(),
            code: self.code,
            name: self.name,
            contact_name: self.contact_name,
            phone: self.phone,
            email: self.email,
            note: self.note,
            status: self.status,
            owner_id: self.owner_id.to_hex(),
            owner: None,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}
