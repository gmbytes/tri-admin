use super::{dto::CaptureInput, models::MetaView, service};
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
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteQuery {
    version: i64,
}
crate::api_routes! {
    prefix "";
    get "/api/v1/versions" => list, "versions.list", "versions", "配置版本列表", Permission, &["versions.read"];
    post "/api/v1/versions" => create, "versions.create", "versions", "捕获配置快照", Permission, &["versions.write"], "所选配置及接口依赖还需各自读取权限";
    post "/api/v1/versions/import-previews" => import_preview, "versions.import.preview", "versions", "预览配置包导入", Permission, &["versions.write"], "所选配置及接口依赖还需各自读取权限；预览不修改配置";
    post "/api/v1/versions/import-previews/{id}/apply" => import_apply, "versions.import.apply", "versions", "应用配置包导入", Permission, &["versions.write"], "首次应用还需各类资源的读取和写入权限；校验预览操作者、校验和、有效期与目标指纹";
    get "/api/v1/versions/{id}" => read, "versions.get", "versions", "版本摘要", Permission, &["versions.read"];
    delete "/api/v1/versions/{id}" => delete, "versions.delete", "versions", "删除版本记录", Permission, &["versions.write"];
    get "/api/v1/versions/{id}/download" => download, "versions.download", "versions", "下载配置发版包", Permission, &["versions.read"], "还需包中各类资源的读取权限";
}
pub fn configured_routes() -> axum::Router<AppState> {
    routes().layer(axum::extract::DefaultBodyLimit::max(10 * 1024 * 1024))
}
async fn import_apply(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<super::import_model::ApplyView>, AppError> {
    actor.require("versions.write")?;
    let input = request::json::<super::import_model::ApplyInput>(body, &state).await?;
    Ok(ApiResponse::success(
        super::apply::apply(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn import_preview(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<super::import_model::PreviewView>), AppError> {
    actor.require("versions.write")?;
    let input = request::json::<super::import_model::PreviewInput>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            super::preview::create(state.require_database()?, Actor::new(&actor)?, input).await?,
        ),
    ))
}
async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<MetaView>, AppError> {
    actor.require("versions.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::list(state.require_database()?, input).await
}
async fn create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<MetaView>), AppError> {
    actor.require("versions.write")?;
    let input = request::json::<CaptureInput>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            service::create(state.require_database()?, Actor::new(&actor)?, input).await?,
        ),
    ))
}
async fn read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<MetaView>, AppError> {
    actor.require("versions.read")?;
    Ok(ApiResponse::success(
        service::get(state.require_database()?, validation::object_id(&id)?).await?,
    ))
}
async fn delete(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    actor.require("versions.write")?;
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
async fn download(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    actor.require("versions.read")?;
    let package = service::package(
        state.require_database()?,
        &Actor::new(&actor)?,
        validation::object_id(&id)?,
    )
    .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"tri-admin-config-{}.json\"",
                    package.meta.code
                ),
            ),
            (header::ETAG, format!("\"{}\"", package.meta.checksum)),
        ],
        package.package_json,
    )
        .into_response())
}
