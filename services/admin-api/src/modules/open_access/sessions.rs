use super::{jwt::Claims, scopes::Scope};
use crate::{
    common::{
        observability,
        transaction::{Failure, fail},
    },
    modules::applications::{models::ApplicationMeta, service},
};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::{Deserialize, Serialize};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserAuthorization {
    scopes: Vec<Scope>,
    auth_version: i64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PrincipalKind {
    Application,
    User,
}
impl PrincipalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::User => "user",
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub schema_version: i32,
    pub application_id: ObjectId,
    pub principal_kind: PrincipalKind,
    pub user_id: Option<ObjectId>,
    pub status: String,
    pub revoked_at: Option<DateTime>,
    pub grant: Vec<Scope>,
    pub credential_version: i64,
    pub user_auth_version: Option<i64>,
    pub refresh_hash: Option<String>,
    pub expires_at: DateTime,
    pub created_at: DateTime,
    pub usage_fence: i64,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refresh {
    #[serde(rename = "_id")]
    pub hash: String,
    pub application_id: ObjectId,
    pub session_id: ObjectId,
    pub user_id: ObjectId,
    pub used_at: Option<DateTime>,
    pub expires_at: DateTime,
    pub created_at: DateTime,
}
pub async fn resolve(
    database: &Database,
    session: &mut ClientSession,
    claims: &Claims,
) -> mongodb::error::Result<(Record, ApplicationMeta, Vec<Scope>)> {
    let invalid = || fail(Failure::Unauthorized);
    let app_id = ObjectId::parse_str(&claims.application_id).map_err(|_| invalid())?;
    let sid = ObjectId::parse_str(&claims.sid).map_err(|_| invalid())?;
    let app = service::lock_app(database, session, app_id)
        .await
        .map_err(|error| {
            if error.get_custom::<Failure>().is_some() {
                invalid()
            } else {
                error
            }
        })?;
    if !app.enabled {
        return Err(invalid());
    }
    let record=database.collection::<Record>("open_sessions").find_one(doc!{"_id":sid,"applicationId":app_id,"schemaVersion":1,"status":"active","revokedAt":null,"expiresAt":{"$gt":DateTime::now()}}).session(&mut *session).await?.ok_or_else(invalid)?;
    if record.principal_kind != claims.principal_kind
        || record.credential_version != claims.credential_version
        || record.user_auth_version != claims.user_auth_version
    {
        return Err(invalid());
    }
    let mut effective = record
        .grant
        .iter()
        .copied()
        .filter(|scope| claims.scopes.contains(scope) && app.allowed_scopes.contains(scope))
        .collect::<Vec<_>>();
    match record.principal_kind {
        PrincipalKind::Application => {
            if claims.sub != app_id.to_hex()
                || record.user_id.is_some()
                || record.credential_version != app.credential_version
            {
                return Err(invalid());
            }
        }
        PrincipalKind::User => {
            let user_id = record.user_id.ok_or_else(invalid)?;
            if claims.sub != user_id.to_hex() {
                return Err(invalid());
            }
            let user = database
                .collection::<UserAuthorization>("application_users")
                .find_one(
                    doc! {"_id":user_id,"applicationId":app_id,"deletedAt":null,"status":"active"},
                )
                .projection(doc! {"scopes":1,"authVersion":1})
                .session(&mut *session)
                .await?
                .ok_or_else(invalid)?;
            if Some(user.auth_version) != record.user_auth_version {
                return Err(invalid());
            }
            effective.retain(|scope| user.scopes.contains(scope) && scope.is_profile());
        }
    }
    database
        .collection::<Document>("open_sessions")
        .update_one(doc! {"_id":sid}, doc! {"$inc":{"usageFence":1i64}})
        .session(session)
        .await?;
    Ok((record, app, effective))
}
pub async fn audit(
    database: &Database,
    session: &mut ClientSession,
    record: &Record,
    action: &str,
    outcome: &str,
) -> mongodb::error::Result<()> {
    database.collection::<Document>("audit_events").insert_one(observability::event(doc!{"category":"openAuthentication","action":action,"actorId":null,"applicationId":record.application_id,"principalKind":record.principal_kind.as_str(),"userId":record.user_id,"sessionId":record.id,"outcome":outcome,"createdAt":DateTime::now()})).session(session).await?;
    Ok(())
}
