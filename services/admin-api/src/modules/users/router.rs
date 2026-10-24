use super::{
    dto::{CreateUser, PasswordInput, PatchUser},
    service,
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        request,
        response::ApiResponse,
        validation,
    },
    modules::{
        auth::AuthContext,
        rbac::{models::UserView, shared::Actor},
    },
    state::AppState,
};
use axum::{
    Router,
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    pub version: i64,
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/users", get(list).post(create))
        .route(
            "/api/v1/users/{id}",
            get(detail).patch(update).delete(delete),
        )
        .route("/api/v1/users/{id}/password", post(reset_password))
}
async fn list(
    State(state): State<AppState>,
    context: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<UserView>, AppError> {
    context.require("users.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::list(state.require_database()?, &Actor::new(&context)?, input).await
}
async fn detail(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<UserView>, AppError> {
    context.require("users.read")?;
    Ok(ApiResponse::success(
        service::get(
            state.require_database()?,
            &Actor::new(&context)?,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn create(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<UserView>), AppError> {
    context.require("users.write")?;
    let input = request::json::<CreateUser>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            service::create(state.require_database()?, Actor::new(&context)?, input).await?,
        ),
    ))
}
async fn update(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<UserView>, AppError> {
    context.require("users.write")?;
    let input = request::json::<PatchUser>(body, &state).await?;
    Ok(ApiResponse::success(
        service::patch(
            state.require_database()?,
            Actor::new(&context)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn delete(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    context.require("users.write")?;
    let Query(input) = query.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    service::delete(
        state.require_database()?,
        Actor::new(&context)?,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn reset_password(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<StatusCode, AppError> {
    context.require("users.resetPassword")?;
    let input = request::json::<PasswordInput>(body, &state).await?;
    service::reset_password(
        state.require_database()?,
        Actor::new(&context)?,
        validation::object_id(&id)?,
        input,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
