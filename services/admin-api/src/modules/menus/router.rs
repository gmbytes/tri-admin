use super::{
    dto::{CreateMenu, PatchMenu},
    service,
};
use crate::{
    common::{error::AppError, request, response::ApiResponse, validation},
    modules::{
        auth::AuthContext,
        rbac::{
            models::MenuView,
            permissions::{CATALOG, Permission},
            shared::Actor,
        },
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
    get "/api/v1/menus" => list, "menus.list", "menus", "菜单列表", Permission, &["menus.read"];
    post "/api/v1/menus" => create, "menus.create", "menus", "创建菜单", Permission, &["menus.write"];
    patch "/api/v1/menus/{id}" => update, "menus.update", "menus", "修改菜单", Permission, &["menus.write"];
    delete "/api/v1/menus/{id}" => delete, "menus.delete", "menus", "删除菜单", Permission, &["menus.write"];
    get "/api/v1/navigation" => navigation, "navigation.get", "menus", "当前导航", Authenticated, &[];
    get "/api/v1/permissions" => permissions, "permissions.list", "permissions", "权限操作目录", Permission, &["permissions.read"];
}
async fn list(
    State(state): State<AppState>,
    context: AuthContext,
) -> Result<ApiResponse<Vec<MenuView>>, AppError> {
    context.require("menus.read")?;
    Ok(ApiResponse::success(
        service::list(state.require_database()?).await?,
    ))
}
async fn navigation(
    State(state): State<AppState>,
    context: AuthContext,
) -> Result<ApiResponse<Vec<MenuView>>, AppError> {
    Ok(ApiResponse::success(
        service::navigation(state.require_database()?, &context).await?,
    ))
}
async fn permissions(context: AuthContext) -> Result<ApiResponse<&'static [Permission]>, AppError> {
    context.require("permissions.read")?;
    Ok(ApiResponse::success(CATALOG))
}
async fn create(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<MenuView>), AppError> {
    context.require("menus.write")?;
    let input = request::json::<CreateMenu>(body, &state).await?;
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
) -> Result<ApiResponse<MenuView>, AppError> {
    context.require("menus.write")?;
    let input = request::json::<PatchMenu>(body, &state).await?;
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
    context.require("menus.write")?;
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
