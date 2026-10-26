pub mod dto;
pub mod jwt;
pub mod repository;
pub mod router;
pub mod service;

use crate::{common::error::AppError, state::AppState};
use axum::{
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts},
};
use mongodb::bson::oid::ObjectId;

#[derive(Clone, Copy)]
pub enum AdminCredential {
    Session { session_id: ObjectId },
    ApiToken { token_id: ObjectId },
}
#[derive(Clone)]
pub struct AuthContext {
    pub user: dto::UserSummary,
    pub credential: AdminCredential,
}

impl AuthContext {
    pub fn require_session(&self) -> Result<ObjectId, AppError> {
        match self.credential {
            AdminCredential::Session { session_id } => Ok(session_id),
            AdminCredential::ApiToken { .. } => Err(AppError::Forbidden),
        }
    }
    pub fn can(&self, permission: &str) -> bool {
        self.user
            .permissions
            .iter()
            .any(|code| code == "*" || code == permission)
    }
    pub fn require(&self, permission: &str) -> Result<(), AppError> {
        if self.can(permission) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

impl FromRequestParts<AppState> for AuthContext {
    type Rejection = AppError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(context) = parts.extensions.get::<AuthContext>() {
            return Ok(context.clone());
        }
        let raw = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;
        let context = if raw.starts_with("tri_at1.") {
            let peer = parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|info| info.0.ip());
            crate::modules::api_tokens::service::authenticate(state, raw, peer).await?
        } else {
            service::authenticate(state, raw).await?
        };
        crate::common::observability::actor(
            ObjectId::parse_str(&context.user.id).map_err(|_| AppError::Internal)?,
        );
        parts.extensions.insert(context.clone());
        Ok(context)
    }
}
