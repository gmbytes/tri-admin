use crate::{Error, State, transaction};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::http::HeaderMap;
use futures_util::FutureExt;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use mongodb::{
    ClientSession,
    bson::{DateTime, Document, doc, oid::ObjectId},
    options::ReturnDocument,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
#[derive(Clone, Deserialize, Serialize)]
pub struct Claims {
    pub sub: String,
    pub sid: String,
    iss: String,
    aud: String,
    iat: i64,
    exp: i64,
    jti: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminView {
    pub id: String,
    pub username: String,
    pub can_write: bool,
    pub data_scope: &'static str,
}
pub struct Actor {
    pub id: ObjectId,
    pub sid: ObjectId,
    pub can_write: bool,
    pub all: bool,
    pub username: String,
}
impl Actor {
    pub fn view(&self) -> AdminView {
        AdminView {
            id: self.id.to_hex(),
            username: self.username.clone(),
            can_write: self.can_write,
            data_scope: if self.all { "all" } else { "self" },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Login {
    pub username: String,
    pub password: String,
}
pub fn claims(state: &State, headers: &HeaderMap) -> Result<Claims, Error> {
    let raw = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty() && v.len() <= 8192)
        .ok_or(Error::Unauthorized)?;
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    validation.set_issuer(&["tri-resource"]);
    validation.set_audience(&[state.schema.resource.as_str()]);
    validation.set_required_spec_claims(&["sub", "iss", "aud", "iat", "exp"]);
    let claims = jsonwebtoken::decode::<Claims>(
        raw,
        &DecodingKey::from_secret(state.config.key.as_bytes()),
        &validation,
    )
    .map_err(|_| Error::Unauthorized)?
    .claims;
    if claims.iat > chrono::Utc::now().timestamp() + 5
        || ObjectId::parse_str(&claims.sub).is_err()
        || ObjectId::parse_str(&claims.sid).is_err()
    {
        return Err(Error::Unauthorized);
    }
    Ok(claims)
}
pub async fn resolve(
    state: &State,
    session: &mut ClientSession,
    claims: &Claims,
    write: bool,
) -> mongodb::error::Result<Actor> {
    let checksum = state
        .schema
        .checksum()
        .map_err(|_| Error::NotReady.mongo())?;
    if state
        .database
        .collection::<Document>("_resource_schema")
        .find_one(doc! {"_id":&state.schema.resource,"version":1,"checksum":checksum})
        .session(&mut *session)
        .await?
        .is_none()
    {
        return Err(Error::NotReady.mongo());
    }
    let now = DateTime::now();
    let id = ObjectId::parse_str(&claims.sub).map_err(|_| Error::Unauthorized.mongo())?;
    let sid = ObjectId::parse_str(&claims.sid).map_err(|_| Error::Unauthorized.mongo())?;
    let user_filter = doc! {"_id":id,"application":&state.schema.resource,"enabled":true};
    let user = if write {
        state
            .database
            .collection::<Document>("_resource_admins")
            .find_one_and_update(user_filter, doc! {"$inc":{"writeFence":1i64}})
            .return_document(ReturnDocument::After)
            .projection(doc! {"username":1,"authVersion":1,"canWrite":1,"scopeAll":1})
            .session(&mut *session)
            .await?
    } else {
        state
            .database
            .collection::<Document>("_resource_admins")
            .find_one(user_filter)
            .projection(doc! {"username":1,"authVersion":1,"canWrite":1,"scopeAll":1})
            .session(&mut *session)
            .await?
    }
    .ok_or_else(|| Error::Unauthorized.mongo())?;
    let version = user
        .get_i64("authVersion")
        .map_err(|_| Error::NotReady.mongo())?;
    if state.database.collection::<Document>("_resource_sessions").find_one(doc!{"_id":sid,"application":&state.schema.resource,"adminId":id,"authVersion":version,"revoked":false,"expiresAt":{"$gt":now}}).session(&mut *session).await?.is_none(){return Err(Error::Unauthorized.mongo())}
    let actor = Actor {
        id,
        sid,
        can_write: user
            .get_bool("canWrite")
            .map_err(|_| Error::NotReady.mongo())?,
        all: user
            .get_bool("scopeAll")
            .map_err(|_| Error::NotReady.mongo())?,
        username: user
            .get_str("username")
            .map_err(|_| Error::NotReady.mongo())?
            .into(),
    };
    if write && !actor.can_write {
        return Err(Error::Forbidden.mongo());
    }
    Ok(actor)
}
pub async fn audit(
    state: &State,
    session: &mut ClientSession,
    actor: ObjectId,
    action: &str,
    id: Option<ObjectId>,
) -> mongodb::error::Result<()> {
    state.database.collection::<Document>("_resource_audit").insert_one(doc!{"application":&state.schema.resource,"actorId":actor,"action":action,"recordId":id,"createdAt":DateTime::now()}).session(session).await?;
    Ok(())
}
async fn hash(password: String) -> Result<String, Error> {
    if password.chars().count() < 12 || password.len() > 1024 {
        return Err(Error::Validation);
    }
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(
                password.as_bytes(),
                &SaltString::generate(&mut rand_core::OsRng),
            )
            .map(|v| v.to_string())
            .map_err(|_| Error::Internal)
    })
    .await
    .map_err(|_| Error::Internal)?
}
static DUMMY: LazyLock<Result<String, Error>> = LazyLock::new(|| {
    let salt = SaltString::encode_b64(b"tri-resource-dummy-salt").map_err(|_| Error::Internal)?;
    Argon2::default()
        .hash_password(b"non-account-placeholder", &salt)
        .map(|v| v.to_string())
        .map_err(|_| Error::Internal)
});
pub async fn provision(
    state: &State,
    username: &str,
    password: &str,
    can_write: bool,
    scope_all: bool,
) -> Result<AdminView, Error> {
    state.ready().await?;
    let username = username.trim();
    if !(3..=64).contains(&username.chars().count())
        || !username
            .chars()
            .all(|v| v.is_alphanumeric() || "._-".contains(v))
    {
        return Err(Error::Validation);
    }
    let hash = hash(password.into()).await?;
    let id = ObjectId::new();
    let view = AdminView {
        id: id.to_hex(),
        username: username.into(),
        can_write,
        data_scope: if scope_all { "all" } else { "self" },
    };
    transaction::run(&state.database,(state.clone(),username.to_owned(),hash),false,move|session,(state,username,hash)|async move{state.database.collection::<Document>("_resource_admins").insert_one(doc!{"_id":id,"application":&state.schema.resource,"username":&*username,"usernameKey":username.to_lowercase(),"passwordHash":&*hash,"canWrite":can_write,"scopeAll":scope_all,"enabled":true,"authVersion":1i64,"writeFence":0i64,"createdAt":DateTime::now()}).session(&mut *session).await?;audit(state,session,id,"admin.create",None).await?;Ok(())}.boxed()).await?;
    Ok(view)
}
async fn rate(state: &State, key: &str, limit: i64) -> Result<(), Error> {
    let now = DateTime::now();
    let reset = DateTime::from_millis(now.timestamp_millis() + 60_000);
    let id = format!(
        "{:x}",
        Sha256::digest(format!("{}:{key}", state.schema.resource))
    );
    let row=state.database.collection::<Document>("_resource_attempts").find_one_and_update(doc!{"_id":id},vec![doc!{"$set":{"count":{"$cond":[{"$gt":["$resetAt",now]},{"$add":[{"$ifNull":["$count",0i64]},1i64]},1i64]},"resetAt":{"$cond":[{"$gt":["$resetAt",now]},"$resetAt",reset]}}}]).upsert(true).return_document(ReturnDocument::After).await?.ok_or(Error::Internal)?;
    if row.get_i64("count").map_err(|_| Error::Internal)? > limit {
        return Err(Error::RateLimited);
    }
    Ok(())
}
pub async fn login(state: &State, input: Login, peer: &str) -> Result<serde_json::Value, Error> {
    state.ready().await?;
    if input.username.trim().is_empty()
        || input.username.chars().count() > 64
        || input.password.is_empty()
        || input.password.len() > 1024
    {
        return Err(Error::Validation);
    }
    rate(state, "global", 120).await?;
    rate(state, &format!("ip:{peer}"), 20).await?;
    let key = input.username.trim().to_lowercase();
    rate(state, &format!("account:{key}"), 10).await?;
    let user = state
        .database
        .collection::<Document>("_resource_admins")
        .find_one(doc! {"application":&state.schema.resource,"usernameKey":&key,"enabled":true})
        .await?;
    let hash = match &user {
        Some(user) => user
            .get_str("passwordHash")
            .map_err(|_| Error::Internal)?
            .to_owned(),
        None => DUMMY.as_ref().map_err(Clone::clone)?.clone(),
    };
    let valid = tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(input.password.as_bytes(), &hash)
                .is_ok()
        })
    })
    .await
    .map_err(|_| Error::Internal)?;
    if !valid {
        return Err(Error::Unauthorized);
    }
    let user = user.ok_or(Error::Unauthorized)?;
    let id = user.get_object_id("_id").map_err(|_| Error::Internal)?;
    let version = user.get_i64("authVersion").map_err(|_| Error::Internal)?;
    let sid = ObjectId::new();
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: id.to_hex(),
        sid: sid.to_hex(),
        iss: "tri-resource".into(),
        aud: state.schema.resource.clone(),
        iat: now,
        exp: now + 900,
        jti: uuid::Uuid::new_v4().to_string(),
    };
    let token = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(state.config.key.as_bytes()),
    )
    .map_err(|_| Error::Internal)?;
    let view = transaction::run(
        &state.database,
        (state.clone(), claims.clone()),
        false,
        |session, (state, claims)| {
            async move {
                // Recheck the password generation after the expensive proof and before creating a session.
                let user = state.database.collection::<Document>("_resource_admins")
                    .find_one_and_update(
                        doc! {"_id":id,"application":&state.schema.resource,"enabled":true,"authVersion":version},
                        doc! {"$inc":{"writeFence":1i64}},
                    )
                    .projection(doc! {"username":1,"canWrite":1,"scopeAll":1})
                    .session(&mut *session).await?
                    .ok_or_else(|| Error::Unauthorized.mongo())?;
                let active = state.database.collection::<Document>("_resource_sessions")
                    .count_documents(doc! {"application":&state.schema.resource,"adminId":id,"revoked":false,"expiresAt":{"$gt":DateTime::now()}})
                    .session(&mut *session).await?;
                if active >= 32 { return Err(Error::RateLimited.mongo()); }
                state.database.collection::<Document>("_resource_sessions")
                    .insert_one(doc! {"_id":sid,"application":&state.schema.resource,"adminId":id,"authVersion":version,"revoked":false,"expiresAt":DateTime::from_millis(claims.exp*1000)})
                    .session(&mut *session).await?;
                audit(state, session, id, "auth.login", None).await?;
                Ok(AdminView {
                    id: id.to_hex(),
                    username: user.get_str("username").map_err(|_| Error::Internal.mongo())?.into(),
                    can_write: user.get_bool("canWrite").map_err(|_| Error::Internal.mongo())?,
                    data_scope: if user.get_bool("scopeAll").map_err(|_| Error::Internal.mongo())? { "all" } else { "self" },
                })
            }.boxed()
        },
    ).await?;
    Ok(serde_json::json!({"accessToken":token,"expiresIn":900,"user":view}))
}
