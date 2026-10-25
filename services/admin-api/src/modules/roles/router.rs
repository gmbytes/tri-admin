use super::{
    dto::{CreateRole, PatchRole},
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
        rbac::{models::RoleView, shared::Actor},
    },
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    pub version: i64,
}
crate::api_routes! {
    prefix "";
    get "/api/v1/roles" => list, "roles.list", "roles", "角色列表", Permission, &["roles.read"];
    post "/api/v1/roles" => create, "roles.create", "roles", "创建角色", Permission, &["roles.write"];
    patch "/api/v1/roles/{id}" => update, "roles.update", "roles", "修改角色", Permission, &["roles.write"];
    delete "/api/v1/roles/{id}" => delete, "roles.delete", "roles", "删除角色", Permission, &["roles.write"];
    get "/api/v1/roles/assignable" => assignable, "roles.assignable", "roles", "可分配角色", Permission, &["roles.assign"];
}
async fn assignable(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<super::dto::RoleChoice>, AppError> {
    actor.require("roles.assign")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::assignable(state.require_database()?, &Actor::new(&actor)?, input).await
}
async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<RoleView>, AppError> {
    actor.require("roles.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::list(state.require_database()?, input).await
}
async fn create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<RoleView>), AppError> {
    actor.require("roles.write")?;
    let input = request::json::<CreateRole>(body, &state).await?;
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
) -> Result<ApiResponse<RoleView>, AppError> {
    actor.require("roles.write")?;
    let input = request::json::<PatchRole>(body, &state).await?;
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
    actor.require("roles.write")?;
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
