use super::{
    AuthContext,
    dto::{LoginInput, RefreshInput, TokenResponse, UserSummary},
    service,
};
use crate::{
    common::{error::AppError, response::ApiResponse},
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{ConnectInfo, FromRequest, Request, State},
    http::StatusCode,
    routing::{get, post},
};
use std::net::SocketAddr;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/password", post(change_password))
}

async fn login(
    State(state): State<AppState>,
    request: Request,
) -> Result<ApiResponse<TokenResponse>, AppError> {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    let Json(input) = Json::<LoginInput>::from_request(request, &state)
        .await
        .map_err(|_| AppError::Validation("请求 JSON 格式或字段无效".into()))?;
    Ok(ApiResponse::success(
        service::login(&state, input, &peer).await?,
    ))
}

async fn refresh(
    State(state): State<AppState>,
    request: Request,
) -> Result<ApiResponse<TokenResponse>, AppError> {
    let Json(input) = Json::<RefreshInput>::from_request(request, &state)
        .await
        .map_err(|_| AppError::Validation("请求 JSON 格式或字段无效".into()))?;
    Ok(ApiResponse::success(
        service::refresh(&state, input.refresh_token).await?,
    ))
}

async fn logout(State(state): State<AppState>, request: Request) -> Result<StatusCode, AppError> {
    let input = crate::common::request::json::<RefreshInput>(request, &state).await?;
    service::logout(&state, input.refresh_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(context: AuthContext) -> ApiResponse<UserSummary> {
    ApiResponse::success(context.user)
}

async fn change_password(
    State(state): State<AppState>,
    context: AuthContext,
    request: Request,
) -> Result<StatusCode, AppError> {
    let input =
        crate::common::request::json::<super::dto::ChangePasswordInput>(request, &state).await?;
    service::change_password(&state, &context, input).await?;
    Ok(StatusCode::NO_CONTENT)
}
