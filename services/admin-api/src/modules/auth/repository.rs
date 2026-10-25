use super::dto::UserSummary;
use crate::common::error::AppError;
use futures_util::TryStreamExt;
use mongodb::{
    Database,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use std::collections::BTreeSet;

pub async fn active_user(database: &Database, id: ObjectId) -> Result<Option<Document>, AppError> {
    Ok(database
        .collection::<Document>("users")
        .find_one(doc! {"_id":id,"status":"active","deletedAt":null})
        .await?)
}

pub async fn summary(database: &Database, user: &Document) -> Result<UserSummary, AppError> {
    let id = user.get_object_id("_id").map_err(|_| AppError::Internal)?;
    let role_ids = user
        .get_array("roleIds")
        .map_err(|_| AppError::Internal)?
        .iter()
        .map(|value| value.as_object_id().ok_or(AppError::Internal))
        .collect::<Result<Vec<_>, _>>()?;
    let roles = database
        .collection::<Document>("roles")
        .find(doc! {"_id":{"$in":&role_ids},"deletedAt":null})
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let mut permissions = BTreeSet::new();
    let mut data_scope = crate::common::model::DataScope::Own;
    let mut scope_roles = BTreeSet::new();
    for role in roles {
        match role.get_str("dataScope").map_err(|_| AppError::Internal)? {
            "all" => data_scope = crate::common::model::DataScope::All,
            "self" => {}
            "roles" => {
                if data_scope != crate::common::model::DataScope::All {
                    data_scope = crate::common::model::DataScope::Roles;
                }
                for value in role
                    .get_array("dataScopeRoleIds")
                    .map_err(|_| AppError::Internal)?
                {
                    scope_roles.insert(value.as_object_id().ok_or(AppError::Internal)?.to_hex());
                }
            }
            _ => return Err(AppError::Internal),
        }
        if let Ok(values) = role.get_array("permissions") {
            for value in values {
                if let Bson::String(code) = value {
                    permissions.insert(code.to_owned());
                }
            }
        }
    }
    Ok(UserSummary {
        id: id.to_hex(),
        username: user
            .get_str("username")
            .map_err(|_| AppError::Internal)?
            .into(),
        display_name: user
            .get_str("displayName")
            .map_err(|_| AppError::Internal)?
            .into(),
        role_ids: role_ids.iter().map(|id| id.to_hex()).collect(),
        permissions: permissions.into_iter().collect(),
        data_scope,
        data_scope_role_ids: if data_scope == crate::common::model::DataScope::All {
            Vec::new()
        } else {
            scope_roles.into_iter().collect()
        },
    })
}
