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
    get "/api/v1/users" => list, "users.list", "users", "用户列表", Permission, &["users.read"];
    post "/api/v1/users" => create, "users.create", "users", "创建用户", Permission, &["roles.assign", "users.write"], "新成员须在可管理范围内";
    get "/api/v1/users/{id}" => detail, "users.get", "users", "用户资料", Permission, &["users.read"];
    patch "/api/v1/users/{id}" => update, "users.update", "users", "修改用户", Permission, &["users.write"], "修改角色时还需 roles.assign，授权不得超出当前权限及数据范围";
    delete "/api/v1/users/{id}" => delete, "users.delete", "users", "删除用户", Permission, &["users.write"];
    post "/api/v1/users/{id}/password" => reset_password, "users.resetPassword", "users", "重置用户密码", Permission, &["users.resetPassword"];
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
