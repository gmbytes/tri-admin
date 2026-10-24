use super::models::RoleRecord;
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        transaction::{Failure, fail},
    },
    modules::auth::AuthContext,
};
use futures_util::TryStreamExt;
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use std::collections::HashSet;

#[derive(Clone)]
pub struct Actor {
    pub id: ObjectId,
    pub permissions: Vec<String>,
    pub scope: DataScope,
}
impl Actor {
    pub fn new(context: &AuthContext) -> Result<Self, AppError> {
        Ok(Self {
            id: ObjectId::parse_str(&context.user.id).map_err(|_| AppError::Internal)?,
            permissions: context.user.permissions.clone(),
            scope: context.user.data_scope,
        })
    }
    pub fn can(&self, code: &str) -> bool {
        self.permissions
            .iter()
            .any(|value| value == "*" || value == code)
    }
    pub fn require_scope(&self, id: ObjectId) -> Result<(), AppError> {
        if self.scope == DataScope::All || self.id == id {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

pub fn ids(values: &[String]) -> Result<Vec<ObjectId>, AppError> {
    if values.is_empty() || values.len() > 32 {
        return Err(AppError::Validation("请选择 1–32 个角色".into()));
    }
    let ids = values
        .iter()
        .map(|value| crate::common::validation::object_id(value))
        .collect::<Result<Vec<_>, _>>()?;
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(AppError::Validation("角色不能重复".into()));
    }
    Ok(ids)
}

pub async fn lock(
    database: &Database,
    key: &str,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    let result = database
        .collection::<Document>("_invariants")
        .update_one(doc! {"_id":key}, doc! {"$inc":{"version":1i64}})
        .session(session)
        .await?;
    if result.matched_count != 1 {
        return Err(fail(Failure::NotReady));
    }
    Ok(())
}

pub async fn roles_for_assignment(
    database: &Database,
    role_ids: &[ObjectId],
    actor: &Actor,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    if !actor.can("roles.assign") {
        return Err(fail(Failure::Forbidden));
    }
    let mut cursor = database
        .collection::<RoleRecord>("roles")
        .find(doc! {"_id":{"$in":role_ids},"deletedAt":null})
        .session(&mut *session)
        .await?;
    let roles = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
    if roles.len() != role_ids.len() {
        return Err(fail(Failure::Validation("所选角色不存在或已删除")));
    }
    for role in roles {
        if role.protected && !actor.can("*")
            || role
                .permissions
                .iter()
                .any(|permission| !actor.can(permission))
            || role.data_scope == DataScope::All && actor.scope != DataScope::All
        {
            return Err(fail(Failure::Forbidden));
        }
    }
    Ok(())
}

pub async fn audit(
    database: &Database,
    actor: &Actor,
    action: &str,
    target: ObjectId,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    database.collection::<Document>("audit_events").insert_one(doc!{"category":"administration","action":action,"actorId":actor.id,"targetId":target,"outcome":"success","createdAt":DateTime::now()}).session(session).await?;
    Ok(())
}

pub async fn revoke_sessions(
    database: &Database,
    user_id: ObjectId,
    session: &mut ClientSession,
) -> mongodb::error::Result<()> {
    database
        .collection::<Document>("sessions")
        .update_many(
            doc! {"userId":user_id,"status":"active"},
            doc! {"$set":{"status":"revoked","revokedAt":DateTime::now()}},
        )
        .session(session)
        .await?;
    Ok(())
}
