use crate::{
    common::{
        model::DataScope,
        transaction::{Failure, fail},
    },
    modules::{
        auth::{AdminCredential, AuthContext},
        rbac::shared::{self, Actor},
    },
};
use futures_util::TryStreamExt;
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use std::collections::BTreeSet;
pub async fn fresh(
    database: &Database,
    session: &mut ClientSession,
    context: &AuthContext,
    write: bool,
) -> mongodb::error::Result<Actor> {
    if database
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"codegen","version":crate::storage::codegen_schema::VERSION})
        .projection(doc! {"_id":1})
        .session(&mut *session)
        .await?
        .is_none()
    {
        return Err(fail(Failure::NotReady));
    }
    if write {
        shared::lock(database, "rbac", session).await?;
        shared::lock(database, "codegen", session).await?;
    }
    let owner = ObjectId::parse_str(&context.user.id).map_err(|_| fail(Failure::Unauthorized))?;
    let now = DateTime::now();
    let mut scopes = None;
    match context.credential{
  AdminCredential::Session{session_id}=>{if database.collection::<Document>("sessions").find_one(doc!{"_id":session_id,"userId":owner,"status":"active","revokedAt":null,"expiresAt":{"$gt":now}}).projection(doc!{"_id":1}).session(&mut *session).await?.is_none(){return Err(fail(Failure::Unauthorized))}},
  AdminCredential::ApiToken{token_id}=>{let token=database.collection::<Document>("api_tokens").find_one(doc!{"_id":token_id,"ownerId":owner,"revokedAt":null,"expiresAt":{"$gt":now}}).projection(doc!{"scopes":1}).session(&mut *session).await?.ok_or_else(||fail(Failure::Unauthorized))?;scopes=Some(token.get_array("scopes").map_err(|_|fail(Failure::NotReady))?.iter().map(|v|v.as_str().map(String::from).ok_or_else(||fail(Failure::NotReady))).collect::<mongodb::error::Result<Vec<_>>>()?);}
 }
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {"_id":owner,"status":"active","deletedAt":null})
        .projection(doc! {"roleIds":1})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::Unauthorized))?;
    let role_ids = user
        .get_array("roleIds")
        .map_err(|_| fail(Failure::NotReady))?
        .iter()
        .map(|v| v.as_object_id().ok_or_else(|| fail(Failure::NotReady)))
        .collect::<mongodb::error::Result<Vec<_>>>()?;
    let mut cursor = database
        .collection::<Document>("roles")
        .find(doc! {"_id":{"$in":&role_ids},"deletedAt":null})
        .projection(doc! {"permissions":1,"dataScope":1,"dataScopeRoleIds":1})
        .session(&mut *session)
        .await?;
    let roles = cursor.stream(session).try_collect::<Vec<_>>().await?;
    let mut permissions = BTreeSet::new();
    let mut scope = DataScope::Own;
    let mut selected = BTreeSet::new();
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
        match role
            .get_str("dataScope")
            .map_err(|_| fail(Failure::NotReady))?
        {
            "all" => scope = DataScope::All,
            "self" => {}
            "roles" => {
                if scope != DataScope::All {
                    scope = DataScope::Roles
                }
                for value in role
                    .get_array("dataScopeRoleIds")
                    .map_err(|_| fail(Failure::NotReady))?
                {
                    selected.insert(
                        value
                            .as_object_id()
                            .ok_or_else(|| fail(Failure::NotReady))?
                            .to_hex(),
                    );
                }
            }
            _ => return Err(fail(Failure::NotReady)),
        }
    }
    let permissions = if let Some(scopes) = scopes {
        scopes
            .into_iter()
            .filter(|code| permissions.contains("*") || permissions.contains(code))
            .collect()
    } else {
        permissions.into_iter().collect()
    };
    let actor = Actor {
        id: owner,
        permissions,
        scope,
        scope_role_ids: if scope == DataScope::All {
            Vec::new()
        } else {
            selected
                .iter()
                .map(|id| ObjectId::parse_str(id).map_err(|_| fail(Failure::NotReady)))
                .collect::<mongodb::error::Result<_>>()?
        },
        role_ids,
    };
    if !actor.can("codegen.read") || write && !actor.can("codegen.write") {
        return Err(fail(Failure::Forbidden));
    }
    Ok(actor)
}
pub async fn owns(
    database: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    owner: ObjectId,
) -> mongodb::error::Result<()> {
    if actor.scope == DataScope::All || actor.id == owner {
        return Ok(());
    }
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {"_id":owner,"deletedAt":null})
        .projection(doc! {"roleIds":1})
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    let roles = user
        .get_array("roleIds")
        .map_err(|_| fail(Failure::NotReady))?
        .iter()
        .map(|v| v.as_object_id().ok_or_else(|| fail(Failure::NotReady)))
        .collect::<mongodb::error::Result<Vec<_>>>()?;
    actor
        .require_scope(owner, &roles)
        .map_err(|_| fail(Failure::NotFound))
}
