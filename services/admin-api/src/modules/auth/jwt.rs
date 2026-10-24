use crate::{common::error::AppError, config::Config};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub sid: String,
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
}

pub fn encode(
    config: &Config,
    user_id: ObjectId,
    session_id: ObjectId,
    session_expiry: i64,
) -> Result<(String, i64), AppError> {
    let now = chrono::Utc::now().timestamp();
    let exp = (now + 900).min(session_expiry / 1000);
    let claims = Claims {
        sub: user_id.to_hex(),
        sid: session_id.to_hex(),
        iss: "tri-admin".into(),
        aud: "tri-admin-desktop".into(),
        iat: now,
        exp,
        jti: uuid::Uuid::new_v4().to_string(),
    };
    let token = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(config.jwt_signing_key().as_bytes()),
    )
    .map_err(|_| AppError::Internal)?;
    Ok((token, exp - now))
}

pub fn decode(config: &Config, raw: &str) -> Result<Claims, AppError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    validation.set_issuer(&["tri-admin"]);
    validation.set_audience(&["tri-admin-desktop"]);
    validation.set_required_spec_claims(&["sub", "iss", "aud", "iat", "exp"]);
    let claims = jsonwebtoken::decode::<Claims>(
        raw,
        &DecodingKey::from_secret(config.jwt_signing_key().as_bytes()),
        &validation,
    )
    .map_err(|_| AppError::Unauthorized)?
    .claims;
    if claims.iat > chrono::Utc::now().timestamp() + 5 {
        return Err(AppError::Unauthorized);
    }
    Ok(claims)
}
