use super::{
    scopes::Scope,
    sessions::{PrincipalKind, Record},
};
use crate::{common::error::AppError, config::Config};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Claims {
    pub sub: String,
    pub sid: String,
    pub application_id: String,
    pub principal_kind: PrincipalKind,
    pub scopes: Vec<Scope>,
    pub credential_version: i64,
    pub user_auth_version: Option<i64>,
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
}
fn audience(kind: PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::Application => "tri-admin-open-application",
        PrincipalKind::User => "tri-admin-open-user",
    }
}
pub fn issue(
    config: &Config,
    record: &Record,
    scopes: Vec<Scope>,
) -> Result<(String, i64), AppError> {
    let now = chrono::Utc::now().timestamp();
    let exp = (now + 900).min(record.expires_at.timestamp_millis() / 1000);
    if exp <= now {
        return Err(AppError::Unauthorized);
    }
    let claims = Claims {
        sub: record.user_id.unwrap_or(record.application_id).to_hex(),
        sid: record.id.to_hex(),
        application_id: record.application_id.to_hex(),
        principal_kind: record.principal_kind,
        scopes,
        credential_version: record.credential_version,
        user_auth_version: record.user_auth_version,
        iss: "tri-admin-open".into(),
        aud: audience(record.principal_kind).into(),
        iat: now,
        exp,
        jti: uuid::Uuid::new_v4().to_string(),
    };
    let token = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(
            config
                .open_jwt_signing_key()
                .ok_or(AppError::OpenAuthUnavailable)?
                .as_bytes(),
        ),
    )
    .map_err(|_| AppError::Internal)?;
    Ok((token, exp - now))
}
pub fn decode(config: &Config, raw: &str) -> Result<Claims, AppError> {
    if raw.len() > 8192 {
        return Err(AppError::Unauthorized);
    }
    let key = config
        .open_jwt_signing_key()
        .ok_or(AppError::OpenAuthUnavailable)?;
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    validation.set_issuer(&["tri-admin-open"]);
    validation.set_audience(&[
        audience(PrincipalKind::Application),
        audience(PrincipalKind::User),
    ]);
    validation.set_required_spec_claims(&["sub", "iss", "aud", "iat", "exp"]);
    let claims =
        jsonwebtoken::decode::<Claims>(raw, &DecodingKey::from_secret(key.as_bytes()), &validation)
            .map_err(|_| AppError::Unauthorized)?
            .claims;
    if claims.aud != audience(claims.principal_kind)
        || claims.iat > chrono::Utc::now().timestamp() + 5
        || claims
            .exp
            .checked_sub(claims.iat)
            .is_none_or(|duration| !(1..=900).contains(&duration))
    {
        return Err(AppError::Unauthorized);
    }
    Ok(claims)
}
