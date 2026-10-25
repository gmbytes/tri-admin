use super::{models::*, service};
use crate::{
    common::{error::AppError, pagination::Page, request, response::ApiResponse, validation},
    modules::{auth::AuthContext, rbac::shared::Actor},
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::StatusCode,
};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyInput {}
pub async fn create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<JobView>), AppError> {
    actor.require("exports.write")?;
    let input = request::json::<CreateInput>(body, &state).await?;
    let (created, row) = service::create(
        state.require_database()?,
        &state.config,
        Actor::new(&actor)?,
        input,
    )
    .await?;
    Ok((
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
pub async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    input: Result<Query<ListInput>, QueryRejection>,
) -> Result<Page<JobView>, AppError> {
    actor.require("exports.read")?;
    let Query(input) = input.map_err(|_| AppError::Validation("导出任务查询参数无效".into()))?;
    service::list(state.require_database()?, &Actor::new(&actor)?, input).await
}
pub async fn get(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<JobView>, AppError> {
    actor.require("exports.read")?;
    Ok(ApiResponse::success(
        service::get_record(
            state.require_database()?,
            &Actor::new(&actor)?,
            validation::object_id(&id)?,
        )
        .await?
        .meta
        .view(mongodb::bson::DateTime::now())?,
    ))
}
pub async fn by_request(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(request): Path<String>,
) -> Result<ApiResponse<JobView>, AppError> {
    actor.require("exports.read")?;
    Ok(ApiResponse::success(
        service::by_request(state.require_database()?, &Actor::new(&actor)?, &request).await?,
    ))
}
pub async fn cancel(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<JobView>, AppError> {
    actor.require("exports.write")?;
    let _ = request::json::<EmptyInput>(body, &state).await?;
    Ok(ApiResponse::success(
        service::cancel(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
pub async fn content(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    actor.require("exports.read")?;
    super::content::download(&state, &Actor::new(&actor)?, validation::object_id(&id)?).await
}
pub async fn retry(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<JobView>), AppError> {
    actor.require("exports.write")?;
    let input = request::json::<super::retry::RetryInput>(body, &state).await?;
    let (created, job) = super::retry::request(
        state.require_database()?,
        Actor::new(&actor)?,
        validation::object_id(&id)?,
        input,
    )
    .await?;
    Ok((
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(job),
    ))
}
pub async fn action(
    State(state): State<AppState>,
    actor: AuthContext,
    Path((id, request)): Path<(String, String)>,
) -> Result<ApiResponse<super::retry::ActionView>, AppError> {
    actor.require("exports.read")?;
    Ok(ApiResponse::success(
        super::retry::receipt(
            state.require_database()?,
            &Actor::new(&actor)?,
            validation::object_id(&id)?,
            &request,
        )
        .await?,
    ))
}
