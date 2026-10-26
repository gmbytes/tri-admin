use super::{
    credential, jwt,
    limits::{Policy, canonical_peer},
    scopes::{self, Scope},
    sessions::{self, PrincipalKind, Record, Refresh},
};
use crate::{
    common::{
        error::AppError,
        password,
        transaction::{self, Failure, fail},
    },
    modules::applications::{
        models::{ApplicationRecord, UserRecord},
        service,
    },
    state::AppState,
};
use futures_util::FutureExt;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationInput {
    pub application_id: String,
    pub secret: String,
    pub scopes: Option<Vec<Scope>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserInput {
    pub application_id: String,
    pub username: String,
    pub password: String,
    pub scopes: Option<Vec<Scope>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshInput {
    pub application_id: String,
    pub refresh_token: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub application_id: String,
    pub principal_kind: PrincipalKind,
    pub scopes: Vec<Scope>,
}
async fn key(state: &AppState) -> Result<(), AppError> {
    state
        .config
        .open_jwt_signing_key()
        .ok_or(AppError::OpenAuthUnavailable)
        .map(|_| ())?;
    if !crate::storage::schema::is_initialized(state.require_database()?).await? {
        return Err(AppError::NotReady);
    }
    Ok(())
}
fn choose(
    requested: Option<Vec<Scope>>,
    available: &[Scope],
    profile: bool,
) -> Result<Vec<Scope>, AppError> {
    let result = scopes::normalize(requested.unwrap_or_else(|| available.to_vec()), profile)?;
    if result.iter().all(|scope| available.contains(scope)) {
        Ok(result)
    } else {
        Err(AppError::Forbidden)
    }
}
fn policy(
    app: &crate::modules::applications::models::ApplicationMeta,
    peer: Option<std::net::IpAddr>,
) -> bool {
    Policy {
        allowed_ips: app.allowed_ips.clone(),
        minute_limit: app.minute_limit,
        daily_quota: app.daily_quota,
    }
    .allows(peer)
}
async fn rate(
    state: &AppState,
    application: ObjectId,
    account: &str,
    peer: Option<std::net::IpAddr>,
) -> Result<(), AppError> {
    let database = state.require_database()?;
    let peer = peer
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unknown".into());
    crate::modules::auth::service::enforce_login_rate(
        database,
        &format!("open:ip:{peer}"),
        state.config.login_limit_per_ip,
    )
    .await?;
    crate::modules::auth::service::enforce_login_rate(
        database,
        &format!("open:account:{application}:{account}"),
        10,
    )
    .await
}
fn result(
    state: &AppState,
    record: Record,
    scopes: Vec<Scope>,
    refresh: Option<String>,
) -> Result<Tokens, AppError> {
    let (access, expires) = jwt::issue(&state.config, &record, scopes.clone())?;
    Ok(Tokens {
        access_token: access,
        refresh_token: refresh,
        expires_in: expires,
        application_id: record.application_id.to_hex(),
        principal_kind: record.principal_kind,
        scopes,
    })
}
pub async fn application(
    state: &AppState,
    input: ApplicationInput,
    peer: Option<std::net::IpAddr>,
) -> Result<Tokens, AppError> {
    key(state).await?;
    let id =
        ObjectId::parse_str(&input.application_id).map_err(|_| AppError::InvalidCredentials)?;
    let peer = peer.map(canonical_peer);
    rate(state, id, "application", peer).await?;
    let database = state.require_database()?;
    let app = database
        .collection::<ApplicationRecord>("applications")
        .find_one(doc! {"_id":id,"deletedAt":null,"enabled":true})
        .await?
        .ok_or(AppError::InvalidCredentials)?;
    if credential::parse("tri_app1", &input.secret) != Some(id)
        || !credential::verify(&input.secret, &app.secret_hash)
    {
        return Err(AppError::InvalidCredentials);
    }
    let requested = input.scopes;
    let record=transaction::run(database,(database.clone(),requested),|session,(database,requested)|async move{let current=service::lock_app(database,session,id).await?;if !current.enabled||current.credential_version!=app.meta.credential_version{return Err(fail(Failure::Unauthorized));}
if !policy(&current,peer){return Err(fail(Failure::Forbidden));}let grant=choose(requested.clone(),&current.allowed_scopes,false).map_err(|_|fail(Failure::Forbidden))?;
        if database.collection::<Document>("open_sessions").count_documents(doc!{"applicationId":id,"principalKind":"application","status":"active","expiresAt":{"$gt":DateTime::now()}}).session(&mut *session).await?>=128{return Err(fail(Failure::RateLimited));}
        let now=DateTime::now();let record=Record{id:ObjectId::new(),schema_version:1,application_id:id,principal_kind:PrincipalKind::Application,user_id:None,status:"active".into(),revoked_at:None,grant,credential_version:current.credential_version,user_auth_version:None,refresh_hash:None,expires_at:DateTime::from_millis(now.timestamp_millis()+900000),created_at:now,usage_fence:0};database.collection::<Record>("open_sessions").insert_one(&record).session(&mut *session).await?;sessions::audit(database,session,&record,"open.auth.application","success").await?;Ok(record)
    }.boxed()).await?;
    let grant = record.grant.clone();
    result(state, record, grant, None)
}
pub async fn login(
    state: &AppState,
    input: UserInput,
    peer: Option<std::net::IpAddr>,
) -> Result<Tokens, AppError> {
    key(state).await?;
    if input.username.chars().count() > 64
        || input.password.len() > 1024
        || input.password.is_empty()
    {
        return Err(AppError::InvalidCredentials);
    }
    let id =
        ObjectId::parse_str(&input.application_id).map_err(|_| AppError::InvalidCredentials)?;
    let username = input.username.trim().to_lowercase();
    let peer = peer.map(canonical_peer);
    rate(state, id, &username, peer).await?;
    let database = state.require_database()?;
    let user = database
        .collection::<UserRecord>("application_users")
        .find_one(
            doc! {"applicationId":id,"usernameKey":username,"status":"active","deletedAt":null},
        )
        .await?;
    let hash = user
        .as_ref()
        .map(|user| user.password_hash.clone())
        .unwrap_or(password::dummy_hash()?.into());
    let valid = password::verify(input.password, hash).await?;
    let user = user.filter(|_| valid).ok_or(AppError::InvalidCredentials)?;
    let sid = ObjectId::new();
    let (raw, refresh_hash) = credential::issue("tri_orf1", sid)?;
    let(record,grant)=transaction::run(database,(database.clone(),input.scopes,refresh_hash),|session,(database,requested,refresh_hash)|async move{let app=service::lock_app(database,session,id).await.map_err(|error|if error.get_custom::<Failure>().is_some(){fail(Failure::Unauthorized)}else{error})?;if !app.enabled{return Err(fail(Failure::Unauthorized));}
if !policy(&app,peer){return Err(fail(Failure::Forbidden));}
        let current=database.collection::<UserRecord>("application_users").find_one(doc!{"_id":user.meta.id,"applicationId":id,"status":"active","deletedAt":null,"authVersion":user.auth_version}).session(&mut *session).await?.ok_or_else(||fail(Failure::Unauthorized))?;
        let available=current.meta.scopes.iter().copied().filter(|scope|app.allowed_scopes.contains(scope)).collect::<Vec<_>>();let grant=choose(requested.clone(),&available,true).map_err(|_|fail(Failure::Forbidden))?;
        if database.collection::<Document>("open_sessions").count_documents(doc!{"applicationId":id,"userId":current.meta.id,"principalKind":"user","status":"active","expiresAt":{"$gt":DateTime::now()}}).session(&mut *session).await?>=32{return Err(fail(Failure::RateLimited));}
        let now=DateTime::now();let expires=DateTime::from_millis(now.timestamp_millis()+7*86400000);let record=Record{id:sid,schema_version:1,application_id:id,principal_kind:PrincipalKind::User,user_id:Some(current.meta.id),status:"active".into(),revoked_at:None,grant:grant.clone(),credential_version:app.credential_version,user_auth_version:Some(current.auth_version),refresh_hash:Some(refresh_hash.clone()),expires_at:expires,created_at:now,usage_fence:0};
        database.collection::<Record>("open_sessions").insert_one(&record).session(&mut *session).await?;database.collection::<Refresh>("open_refresh_tokens").insert_one(Refresh{hash:refresh_hash.clone(),application_id:id,session_id:sid,user_id:current.meta.id,used_at:None,expires_at:expires,created_at:now}).session(&mut *session).await?;sessions::audit(database,session,&record,"open.auth.login","success").await?;Ok((record,grant))
    }.boxed()).await?;
    result(state, record, grant, Some(raw))
}
pub async fn refresh(
    state: &AppState,
    input: RefreshInput,
    peer: Option<std::net::IpAddr>,
) -> Result<Tokens, AppError> {
    key(state).await?;
    let app = ObjectId::parse_str(&input.application_id).map_err(|_| AppError::Unauthorized)?;
    let sid = credential::parse("tri_orf1", &input.refresh_token).ok_or(AppError::Unauthorized)?;
    let old = credential::hash(&input.refresh_token);
    let (raw, new) = credential::issue("tri_orf1", sid)?;
    let peer = peer.map(canonical_peer);
    rate(state, app, &format!("refresh:{sid}"), peer).await?;
    let database = state.require_database()?;
    let outcome=transaction::run(database,(database.clone(),old,new),|session,(database,old,new)|async move{let parent=service::lock_app(database,session,app).await.map_err(|error|if error.get_custom::<Failure>().is_some(){fail(Failure::Unauthorized)}else{error})?;if !parent.enabled{return Err(fail(Failure::Unauthorized));}
        let token=database.collection::<Refresh>("open_refresh_tokens").find_one(doc!{"_id":old.as_str(),"applicationId":app,"sessionId":sid,"expiresAt":{"$gt":DateTime::now()}}).session(&mut *session).await?.ok_or_else(||fail(Failure::Unauthorized))?;
        let mut record=database.collection::<Record>("open_sessions").find_one(doc!{"_id":sid,"applicationId":app,"principalKind":"user","schemaVersion":1,"status":"active","revokedAt":null,"expiresAt":{"$gt":DateTime::now()}}).session(&mut *session).await?.ok_or_else(||fail(Failure::Unauthorized))?;
        let user=database.collection::<UserRecord>("application_users").find_one(doc!{"_id":token.user_id,"applicationId":app,"status":"active","deletedAt":null,"authVersion":record.user_auth_version}).session(&mut *session).await?.ok_or_else(||fail(Failure::Unauthorized))?;
        if record.user_id!=Some(token.user_id){return Err(fail(Failure::Unauthorized));}
        if !policy(&parent,peer){return Err(fail(Failure::Forbidden));}
        let now=DateTime::now();if token.used_at.is_some()||record.refresh_hash.as_deref()!=Some(old.as_str()){database.collection::<Document>("open_sessions").update_one(doc!{"_id":sid},doc!{"$set":{"status":"revoked","revokedAt":now}}).session(&mut *session).await?;sessions::audit(database,session,&record,"open.auth.refreshReuse","revoked").await?;return Ok(None);}
        let grant=record.grant.iter().copied().filter(|scope|parent.allowed_scopes.contains(scope)&&user.meta.scopes.contains(scope)&&scope.is_profile()).collect::<Vec<_>>();
        database.collection::<Document>("open_refresh_tokens").update_one(doc!{"_id":old.as_str()},doc!{"$set":{"usedAt":now}}).session(&mut *session).await?;database.collection::<Refresh>("open_refresh_tokens").insert_one(Refresh{hash:new.clone(),application_id:app,session_id:sid,user_id:token.user_id,used_at:None,expires_at:record.expires_at,created_at:now}).session(&mut *session).await?;database.collection::<Document>("open_sessions").update_one(doc!{"_id":sid},doc!{"$set":{"refreshHash":new.as_str()}}).session(&mut *session).await?;record.refresh_hash=Some(new.clone());Ok(Some((record,grant)))
    }.boxed()).await?;
    let (record, grant) = outcome.ok_or(AppError::Unauthorized)?;
    result(state, record, grant, Some(raw))
}
pub async fn logout_user(state: &AppState, input: RefreshInput) -> Result<(), AppError> {
    key(state).await?;
    let app = ObjectId::parse_str(&input.application_id).map_err(|_| AppError::Unauthorized)?;
    let Some(sid) = credential::parse("tri_orf1", &input.refresh_token) else {
        return Ok(());
    };
    let hash = credential::hash(&input.refresh_token);
    let database = state.require_database()?;
    transaction::run(
        database,
        (database.clone(), hash),
        |session, (database, hash)| {
            async move {
                let Some(_) = database
                    .collection::<Refresh>("open_refresh_tokens")
                    .find_one(doc! {"_id":hash.as_str(),"applicationId":app,"sessionId":sid})
                    .session(&mut *session)
                    .await?
                else {
                    return Ok(());
                };
                if let Some(record) = database
                    .collection::<Record>("open_sessions")
                    .find_one(
                        doc! {"_id":sid,"applicationId":app,"schemaVersion":1,"status":"active"},
                    )
                    .session(&mut *session)
                    .await?
                {
                    database
                        .collection::<Document>("open_sessions")
                        .update_one(
                            doc! {"_id":sid},
                            doc! {"$set":{"status":"revoked","revokedAt":DateTime::now()}},
                        )
                        .session(&mut *session)
                        .await?;
                    sessions::audit(database, session, &record, "open.auth.logout", "success")
                        .await?;
                }
                Ok(())
            }
            .boxed()
        },
    )
    .await
}
pub async fn logout_application(state: &AppState, raw: &str) -> Result<(), AppError> {
    key(state).await?;
    let claims = jwt::decode(&state.config, raw)?;
    if claims.principal_kind != PrincipalKind::Application {
        return Err(AppError::Forbidden);
    }
    let app = ObjectId::parse_str(&claims.application_id).map_err(|_| AppError::Unauthorized)?;
    let sid = ObjectId::parse_str(&claims.sid).map_err(|_| AppError::Unauthorized)?;
    if claims.sub != app.to_hex() {
        return Err(AppError::Unauthorized);
    }
    let database = state.require_database()?;
    transaction::run(database,database.clone(),|session,database|async move{if let Some(record)=database.collection::<Record>("open_sessions").find_one(doc!{"_id":sid,"applicationId":app,"principalKind":"application","schemaVersion":1,"status":"active"}).session(&mut *session).await?{database.collection::<Document>("open_sessions").update_one(doc!{"_id":sid},doc!{"$set":{"status":"revoked","revokedAt":DateTime::now()}}).session(&mut *session).await?;sessions::audit(database,session,&record,"open.auth.applicationLogout","success").await?;}Ok(())}.boxed()).await
}
