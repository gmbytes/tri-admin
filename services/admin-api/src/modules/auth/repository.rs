use super::dto::UserSummary;
use crate::common::error::AppError;
use futures_util::TryStreamExt;
use mongodb::{
    ClientSession, Database,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use std::collections::BTreeSet;

/// Revalidates interactive management authority inside the caller's transaction.
pub async fn interactive_permissions(
    database: &Database,
    session: &mut ClientSession,
    owner: ObjectId,
    session_id: ObjectId,
    required: &[&str],
) -> mongodb::error::Result<std::collections::HashSet<String>> {
    use crate::common::transaction::{Failure, fail};
    if database.collection::<Document>("sessions").find_one(doc!{"_id":session_id,"userId":owner,"status":"active","revokedAt":null,"expiresAt":{"$gt":mongodb::bson::DateTime::now()}}).session(&mut *session).await?.is_none(){return Err(fail(Failure::Unauthorized));}
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {"_id":owner,"status":"active","deletedAt":null})
        .projection(doc! {"roleIds":1})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Unauthorized))?;
    let ids = user
        .get_array("roleIds")
        .map_err(|_| fail(Failure::NotReady))?;
    let mut cursor = database
        .collection::<Document>("roles")
        .find(doc! {"_id":{"$in":ids},"deletedAt":null})
        .projection(doc! {"permissions":1})
        .session(&mut *session)
        .await?;
    let roles = cursor.stream(session).try_collect::<Vec<_>>().await?;
    let mut permissions = std::collections::HashSet::new();
    for role in roles {
        for value in role
            .get_array("permissions")
            .map_err(|_| fail(Failure::NotReady))?
        {
            permissions.insert(
                value
                    .as_str()
                    .ok_or_else(|| fail(Failure::NotReady))?
                    .to_owned(),
            );
        }
    }
    if !permissions.contains("*") && required.iter().any(|code| !permissions.contains(*code)) {
        return Err(fail(Failure::Forbidden));
    }
    Ok(permissions)
}

pub async fn active_user(database: &Database, id: ObjectId) -> Result<Option<Document>, AppError> {
    Ok(database
        .collection::<Document>("users")
        .find_one(doc! {"_id":id,"status":"active","deletedAt":null})
        .projection(doc! {"_id":1,"username":1,"displayName":1,"roleIds":1})
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
