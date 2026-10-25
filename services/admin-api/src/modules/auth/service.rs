use super::{
    AuthContext,
    dto::{LoginInput, TokenResponse},
    jwt, repository,
};
use crate::{
    common::{error::AppError, password},
    state::AppState,
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
    options::ReturnDocument,
};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

fn token_hash(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

fn random_token() -> Result<String, AppError> {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| AppError::Internal)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

async fn enforce_login_rate(database: &Database, key: &str, limit: i64) -> Result<(), AppError> {
    let now = DateTime::now();
    let reset = DateTime::from_millis(now.timestamp_millis() + 60_000);
    let attempts=database.collection::<Document>("login_attempts").find_one_and_update(doc!{"_id":token_hash(key)},vec![doc!{"$set":{
        "count":{"$cond":[{"$gt":["$resetAt",now]},{"$add":[{"$ifNull":["$count",0i64]},1i64]},1i64]},
        "resetAt":{"$cond":[{"$gt":["$resetAt",now]},"$resetAt",reset]}
    }}]).upsert(true).return_document(ReturnDocument::After).await?.ok_or(AppError::Internal)?;
    if attempts.get_i64("count").map_err(|_| AppError::Internal)? > limit {
        return Err(AppError::RateLimited);
    }
    Ok(())
}

pub async fn login(
    state: &AppState,
    input: LoginInput,
    peer: &str,
) -> Result<TokenResponse, AppError> {
    if input.username.trim().is_empty()
        || input.username.trim().chars().count() > 64
        || input.password.is_empty()
        || input.password.len() > 1024
    {
        return Err(AppError::Validation("用户名或密码格式无效".into()));
    }
    let database = state.require_database()?;
    if !crate::storage::schema::is_initialized(database).await? {
        return Err(AppError::NotReady);
    }
    let username_key = input.username.trim().to_lowercase();
    crate::common::observability::login_username(&username_key);
    let now = DateTime::now();
    enforce_login_rate(
        database,
        &format!("ip:{peer}"),
        state.config.login_limit_per_ip,
    )
    .await?;
    enforce_login_rate(database, &format!("account:{username_key}"), 10).await?;
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {"usernameKey":&username_key,"status":"active","deletedAt":null})
        .await?;
    let hash = match &user {
        Some(user) => user
            .get_str("passwordHash")
            .map_err(|_| AppError::Internal)?,
        None => password::dummy_hash()?,
    }
    .to_owned();
    let verified = password::verify(input.password, hash).await?;
    let mut event = crate::common::observability::event(
        doc! {"category":"authentication","action":"auth.login","username":&username_key,"ip":peer,"createdAt":now},
    );
    if !verified || user.is_none() {
        event.insert("outcome", "failure");
        event.insert("errorCode", "INVALID_CREDENTIALS");
        database
            .collection::<Document>("audit_events")
            .insert_one(event)
            .await?;
        crate::common::observability::login_recorded();
        return Err(AppError::InvalidCredentials);
    }
    let user = user.ok_or(AppError::InvalidCredentials)?;
    let user_id = user.get_object_id("_id").map_err(|_| AppError::Internal)?;
    let summary = repository::summary(database, &user).await?;
    let session_id = ObjectId::new();
    let refresh_token = random_token()?;
    let refresh_hash = token_hash(&refresh_token);
    let expires = DateTime::from_millis(now.timestamp_millis() + 7 * 24 * 60 * 60 * 1000);
    let session_doc = doc! {"_id":session_id,"userId":user_id,"status":"active","refreshHash":&refresh_hash,"createdAt":now,"expiresAt":expires,"revokedAt":null};
    let refresh_doc = doc! {"_id":refresh_hash,"sessionId":session_id,"userId":user_id,"usedAt":null,"createdAt":now,"expiresAt":expires};
    event.insert("outcome", "success");
    event.insert("actorId", user_id);
    event.insert("sessionId", session_id);
    let mut transaction = database.client().start_session().await?;
    transaction
        .start_transaction()
        .and_run(
            (database.clone(), session_doc, refresh_doc, event),
            |session, (database, session_doc, refresh_doc, event)| {
                async move {
                    database
                        .collection::<Document>("sessions")
                        .insert_one(session_doc.clone())
                        .session(&mut *session)
                        .await?;
                    database
                        .collection::<Document>("refresh_tokens")
                        .insert_one(refresh_doc.clone())
                        .session(&mut *session)
                        .await?;
                    database
                        .collection::<Document>("audit_events")
                        .insert_one(event.clone())
                        .session(&mut *session)
                        .await?;
                    Ok(())
                }
                .boxed()
            },
        )
        .await?;
    crate::common::observability::login_recorded();
    let (access_token, expires_in) = jwt::encode(
        &state.config,
        user_id,
        session_id,
        expires.timestamp_millis(),
    )?;
    Ok(TokenResponse {
        access_token,
        refresh_token,
        expires_in,
        user: summary,
    })
}

pub async fn authenticate(state: &AppState, raw: &str) -> Result<AuthContext, AppError> {
    let claims = jwt::decode(&state.config, raw)?;
    let user_id = ObjectId::parse_str(&claims.sub).map_err(|_| AppError::Unauthorized)?;
    let session_id = ObjectId::parse_str(&claims.sid).map_err(|_| AppError::Unauthorized)?;
    let database = state.require_database()?;
    if !crate::storage::schema::is_initialized(database).await? {
        return Err(AppError::NotReady);
    }
    let session=database.collection::<Document>("sessions").find_one(doc!{"_id":session_id,"userId":user_id,"status":"active","revokedAt":null,"expiresAt":{"$gt":DateTime::now()}}).await?;
    if session.is_none() {
        return Err(AppError::Unauthorized);
    }
    let user = repository::active_user(database, user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    Ok(AuthContext {
        user: repository::summary(database, &user).await?,
        session_id,
    })
}

enum RefreshResult {
    Rotated {
        user_id: ObjectId,
        session_id: ObjectId,
        expires: i64,
    },
    Reused,
}

pub async fn refresh(state: &AppState, raw: String) -> Result<TokenResponse, AppError> {
    if raw.len() != 64 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::Unauthorized);
    }
    let database = state.require_database()?;
    let previous_hash = token_hash(&raw);
    let refresh_token = random_token()?;
    let replacement_hash = token_hash(&refresh_token);
    let mut transaction = database.client().start_session().await?;
    let result=transaction.start_transaction().and_run((database.clone(),previous_hash,replacement_hash),|session,(database,previous_hash,replacement_hash)|async move {
        let now=DateTime::now();
        let invalid=||mongodb::error::Error::custom("invalid-refresh-token");
        let tokens=database.collection::<Document>("refresh_tokens");
        let token=tokens.find_one(doc!{"_id":previous_hash.as_str(),"expiresAt":{"$gt":now}}).session(&mut *session).await?.ok_or_else(invalid)?;
        let session_id=token.get_object_id("sessionId").map_err(|_|invalid())?;
        let sessions=database.collection::<Document>("sessions");
        let current=sessions.find_one(doc!{"_id":session_id,"status":"active","revokedAt":null,"expiresAt":{"$gt":now}}).session(&mut *session).await?.ok_or_else(invalid)?;
        let user_id=current.get_object_id("userId").map_err(|_|invalid())?;
        let user=database.collection::<Document>("users").find_one(doc!{"_id":user_id,"status":"active","deletedAt":null}).session(&mut *session).await?;
        if user.is_none(){return Err(invalid());}
        crate::common::observability::actor(user_id);
        let unused=token.get("usedAt").is_none_or(|value|matches!(value,mongodb::bson::Bson::Null));
        if !unused || current.get_str("refreshHash").ok()!=Some(previous_hash.as_str()) {
            sessions.update_one(doc!{"_id":session_id},doc!{"$set":{"status":"revoked","revokedAt":now}}).session(&mut *session).await?;
            database.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"authentication","action":"auth.refreshReuse","actorId":user_id,"sessionId":session_id,"outcome":"revoked","createdAt":now})).session(&mut *session).await?;
            // Commit the revocation before reporting the rejected request.
            return Ok(RefreshResult::Reused);
        }
        let expires=*current.get_datetime("expiresAt").map_err(|_|invalid())?;
        tokens.update_one(doc!{"_id":previous_hash.as_str()},doc!{"$set":{"usedAt":now}}).session(&mut *session).await?;
        tokens.insert_one(doc!{"_id":replacement_hash.as_str(),"sessionId":session_id,"userId":user_id,"usedAt":null,"createdAt":now,"expiresAt":expires}).session(&mut *session).await?;
        sessions.update_one(doc!{"_id":session_id},doc!{"$set":{"refreshHash":replacement_hash.as_str(),"lastSeenAt":now}}).session(&mut *session).await?;
        Ok(RefreshResult::Rotated{user_id,session_id,expires:expires.timestamp_millis()})
    }.boxed()).await;
    let result = match result {
        Ok(result) => result,
        Err(error) if error.get_custom::<&str>() == Some(&"invalid-refresh-token") => {
            return Err(AppError::Unauthorized);
        }
        Err(error) => return Err(error.into()),
    };
    match result {
        RefreshResult::Reused => Err(AppError::Unauthorized),
        RefreshResult::Rotated {
            user_id,
            session_id,
            expires,
        } => {
            let user = repository::active_user(database, user_id)
                .await?
                .ok_or(AppError::Unauthorized)?;
            let (access_token, expires_in) =
                jwt::encode(&state.config, user_id, session_id, expires)?;
            Ok(TokenResponse {
                access_token,
                refresh_token,
                expires_in,
                user: repository::summary(database, &user).await?,
            })
        }
    }
}

pub async fn logout(state: &AppState, raw: String) -> Result<(), AppError> {
    if raw.len() != 64 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(());
    }
    let database = state.require_database()?;
    let Some(token) = database
        .collection::<Document>("refresh_tokens")
        .find_one(doc! {"_id":token_hash(&raw)})
        .await?
    else {
        return Ok(());
    };
    let session_id = token
        .get_object_id("sessionId")
        .map_err(|_| AppError::Internal)?;
    let user_id = token
        .get_object_id("userId")
        .map_err(|_| AppError::Internal)?;
    crate::common::observability::actor(user_id);
    let mut transaction = database.client().start_session().await?;
    transaction.start_transaction().and_run(database.clone(),|session,database|async move{
        let now=DateTime::now();
        let update=database.collection::<Document>("sessions").update_one(doc!{"_id":session_id,"status":"active"},doc!{"$set":{"status":"revoked","revokedAt":now}}).session(&mut *session).await?;
        if update.modified_count>0 {database.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"authentication","action":"auth.logout","actorId":user_id,"sessionId":session_id,"outcome":"success","createdAt":now})).session(&mut *session).await?;}
        Ok(())
    }.boxed()).await?;
    Ok(())
}

pub async fn change_password(
    state: &AppState,
    context: &AuthContext,
    input: super::dto::ChangePasswordInput,
) -> Result<(), AppError> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Credentials {
        password_hash: String,
        version: i64,
    }
    crate::common::validation::password(&input.new_password)?;
    if input.current_password.is_empty() || input.current_password.len() > 1024 {
        return Err(AppError::Validation("当前密码格式无效".into()));
    }
    if input.current_password == input.new_password {
        return Err(AppError::Validation("新密码不能与当前密码相同".into()));
    }
    let database = state.require_database()?;
    let id = ObjectId::parse_str(&context.user.id).map_err(|_| AppError::Internal)?;
    enforce_login_rate(database, &format!("password:{id}"), 5).await?;
    let credentials = database
        .collection::<Credentials>("users")
        .find_one(doc! {"_id":id,"status":"active","deletedAt":null})
        .await?
        .ok_or(AppError::Unauthorized)?;
    if !password::verify(input.current_password, credentials.password_hash).await? {
        return Err(AppError::InvalidCredentials);
    }
    crate::modules::users::service::reset_password(
        database,
        crate::modules::rbac::shared::Actor::new(context)?,
        id,
        crate::modules::users::dto::PasswordInput {
            version: credentials.version,
            password: input.new_password,
        },
    )
    .await
}
