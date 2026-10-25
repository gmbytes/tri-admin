use crate::common::{error::AppError, model::DataScope};
use mongodb::bson::{DateTime, oid::ObjectId};
use serde::{Deserialize, Serialize};

fn time(value: DateTime) -> Result<String, AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.timestamp_millis())
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::Internal)
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub username: String,
    pub display_name: String,
    pub role_ids: Vec<ObjectId>,
    pub status: String,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserView {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role_ids: Vec<String>,
    pub roles: Vec<RoleBrief>,
    pub status: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl UserRecord {
    pub fn view(self) -> Result<UserView, AppError> {
        Ok(UserView {
            id: self.id.to_hex(),
            username: self.username,
            display_name: self.display_name,
            role_ids: self.role_ids.into_iter().map(ObjectId::to_hex).collect(),
            roles: Vec::new(),
            status: self.status,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}

#[derive(Serialize)]
pub struct RoleBrief {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub name: String,
    pub slug: String,
    pub permissions: Vec<String>,
    pub data_scope: DataScope,
    pub protected: bool,
    pub version: i64,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleView {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub permissions: Vec<String>,
    pub data_scope: DataScope,
    pub protected: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}
impl RoleRecord {
    pub fn view(self) -> Result<RoleView, AppError> {
        Ok(RoleView {
            id: self.id.to_hex(),
            name: self.name,
            slug: self.slug,
            permissions: self.permissions,
            data_scope: self.data_scope,
            protected: self.protected,
            version: self.version,
            created_at: time(self.created_at)?,
            updated_at: time(self.updated_at)?,
        })
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuRecord {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub label: String,
    pub path: String,
    pub view_key: Option<String>,
    pub icon: Option<String>,
    pub permission: Option<String>,
    pub parent_id: Option<ObjectId>,
    pub sort: i32,
    pub enabled: bool,
    pub version: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuView {
    pub id: String,
    pub label: String,
    pub path: String,
    pub view_key: Option<String>,
    pub icon: Option<String>,
    pub permission: Option<String>,
    pub parent_id: Option<String>,
    pub sort: i32,
    pub enabled: bool,
    pub version: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MenuView>,
}
impl MenuRecord {
    pub fn view(self) -> MenuView {
        MenuView {
            id: self.id.to_hex(),
            label: self.label,
            path: self.path,
            view_key: self.view_key,
            icon: self.icon,
            permission: self.permission,
            parent_id: self.parent_id.map(ObjectId::to_hex),
            sort: self.sort,
            enabled: self.enabled,
            version: self.version,
            children: vec![],
        }
    }
}
