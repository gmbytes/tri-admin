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

pub struct AuthContext {
    pub user: dto::UserSummary,
    pub session_id: ObjectId,
}

impl AuthContext {
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
        let raw = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;
        service::authenticate(state, raw).await
    }
}
