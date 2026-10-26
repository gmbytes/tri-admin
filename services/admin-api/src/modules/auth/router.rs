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
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
};
use std::net::SocketAddr;

crate::api_routes! {
    prefix "/api/v1/auth";
    post "/login" => login, "auth.login", "auth", "登录", Public, &[];
    post "/refresh" => refresh, "auth.refresh", "auth", "刷新会话", RefreshCredential, &[];
    post "/logout" => logout, "auth.logout", "auth", "注销会话", RefreshCredential, &[];
    get "/me" => me, "auth.me", "auth", "当前用户", Authenticated, &[];
    post "/password" => change_password, "auth.changePassword", "auth", "修改本人密码", Authenticated, &[], "仅交互会话";
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
    let input = crate::common::request::json::<LoginInput>(request, &state).await?;
    Ok(ApiResponse::success(
        service::login(&state, input, &peer).await?,
    ))
}

async fn refresh(
    State(state): State<AppState>,
    request: Request,
) -> Result<ApiResponse<TokenResponse>, AppError> {
    let input = crate::common::request::json::<RefreshInput>(request, &state).await?;
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
