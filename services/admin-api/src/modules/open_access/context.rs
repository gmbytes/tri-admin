use super::{
    jwt, open_limits,
    scopes::Scope,
    sessions::{self, PrincipalKind, Record},
};
use crate::{common::error::AppError, state::AppState};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{header::AUTHORIZATION, request::Parts},
};
use mongodb::bson::oid::ObjectId;
#[derive(Clone)]
pub struct OpenContext {
    pub claims: jwt::Claims,
    pub session: Record,
    pub scopes: Vec<Scope>,
}
impl OpenContext {
    pub fn application_id(&self) -> ObjectId {
        self.session.application_id
    }
    pub fn require(&self, kind: PrincipalKind, scope: Scope) -> Result<(), AppError> {
        if self.session.principal_kind == kind && self.scopes.contains(&scope) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}
impl FromRequestParts<AppState> for OpenContext {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        if let Some(context) = parts.extensions.get::<Self>() {
            return Ok(context.clone());
        }
        let raw = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;
        let claims = jwt::decode(&state.config, raw)?;
        let peer = parts
            .extensions
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|v| super::limits::canonical_peer(v.0.ip()));
        let context = open_limits::reserve(state, claims, peer).await?;
        parts.extensions.insert(context.clone());
        Ok(context)
    }
}
pub async fn authorize(
    database: &mongodb::Database,
    transaction: &mut mongodb::ClientSession,
    context: &OpenContext,
    kind: PrincipalKind,
    scope: Scope,
) -> mongodb::error::Result<(
    Record,
    crate::modules::applications::models::ApplicationMeta,
)> {
    let (record, app, scopes) = sessions::resolve(database, transaction, &context.claims).await?;
    if record.principal_kind != kind || !scopes.contains(&scope) {
        return Err(crate::common::transaction::fail(
            crate::common::transaction::Failure::Forbidden,
        ));
    }
    Ok((record, app))
}
