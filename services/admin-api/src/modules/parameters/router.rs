use super::{
    dto::{CreateParameter, PatchParameter},
    models::ParameterView,
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
struct DeleteQuery {
    version: i64,
}

crate::api_routes! {
    prefix "";
    get "/api/v1/parameters" => list, "parameters.list", "parameters", "参数列表", Permission, &["parameters.read"];
    post "/api/v1/parameters" => create, "parameters.create", "parameters", "创建参数", Permission, &["parameters.write"];
    get "/api/v1/parameters/{id}" => read, "parameters.get", "parameters", "参数资料", Permission, &["parameters.read"];
    patch "/api/v1/parameters/{id}" => update, "parameters.update", "parameters", "修改参数", Permission, &["parameters.write"];
    delete "/api/v1/parameters/{id}" => delete, "parameters.delete", "parameters", "删除参数", Permission, &["parameters.write"];
}
async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<ParameterView>, AppError> {
    actor.require("parameters.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::list(state.require_database()?, input).await
}
async fn read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ParameterView>, AppError> {
    actor.require("parameters.read")?;
    Ok(ApiResponse::success(
        service::get(state.require_database()?, validation::object_id(&id)?).await?,
    ))
}
async fn create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ParameterView>), AppError> {
    actor.require("parameters.write")?;
    let input = request::json::<CreateParameter>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            service::create(state.require_database()?, Actor::new(&actor)?, input).await?,
        ),
    ))
}
async fn update(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<ParameterView>, AppError> {
    actor.require("parameters.write")?;
    let input = request::json::<PatchParameter>(body, &state).await?;
    Ok(ApiResponse::success(
        service::patch(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn delete(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    actor.require("parameters.write")?;
    let Query(input) = query.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    service::delete(
        state.require_database()?,
        Actor::new(&actor)?,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
