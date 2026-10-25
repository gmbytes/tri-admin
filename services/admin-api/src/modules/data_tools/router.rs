use super::{exports, formats, imports, models::*, registry};
use crate::{
    common::{error::AppError, request, response::ApiResponse, validation},
    modules::{auth::AuthContext, rbac::shared::Actor},
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::{HeaderValue, StatusCode, header},
};

crate::api_routes! {
    prefix "";
    get "/api/v1/data-tools/resources" => resources, "dataTools.resources", "dataTools", "数据工具资源模板", Permission, &["exports.read"], "仅显示具备资源读取权限的字段白名单";
    post "/api/v1/data-tools/template" => template, "dataTools.template", "dataTools", "下载新资源表格模板", Permission, &["exports.read"];
    post "/api/v1/data-tools/inspect" => inspect, "dataTools.inspect", "dataTools", "读取导入文件表头", Permission, &["exports.write"], "还需资源读取和写入权限；只返回表头、行数和原始内容摘要，不创建导入记录";
    post "/api/v1/data-tools/export" => export, "dataTools.export", "dataTools", "导出资源表格", Permission, &["exports.write"], "还需资源读取权限；用户数据范围在限制行数前过滤";
    get "/api/v1/data-tools/imports" => list, "dataTools.import.list", "dataTools", "我的最近导入", Permission, &["exports.read"];
    post "/api/v1/data-tools/imports" => preview, "dataTools.import.preview", "dataTools", "预览表格导入", Permission, &["exports.write"], "还需资源读取和写入权限；UUID 请求标识绑定内容与映射";
    get "/api/v1/data-tools/imports/{id}" => read, "dataTools.import.get", "dataTools", "查询导入逐行结果", Permission, &["exports.read"], "仅原操作者可读，还需资源读取权限";
    post "/api/v1/data-tools/imports/{id}/apply" => apply, "dataTools.import.apply", "dataTools", "应用导入批次", Permission, &["exports.write"], "最多100行，每行数据与回执原子提交；仅原操作者并重新检查资源权限";
}
pub fn configured_routes() -> axum::Router<AppState> {
    routes()
        .layer(axum::extract::DefaultBodyLimit::max(9 * 1024 * 1024))
        .layer(axum::middleware::map_response(private_response))
}
async fn private_response(mut response: axum::response::Response) -> axum::response::Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
async fn resources(
    actor: AuthContext,
) -> Result<ApiResponse<Vec<&'static registry::Resource>>, AppError> {
    actor.require("exports.read")?;
    let actor = Actor::new(&actor)?;
    Ok(ApiResponse::success(
        registry::RESOURCES
            .iter()
            .filter(|r| actor.can(r.read_permission))
            .collect(),
    ))
}
async fn template(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<ApiResponse<exports::Download>, AppError> {
    actor.require("exports.read")?;
    let input = request::json::<ExportInput>(body, &state).await?;
    let actor = Actor::new(&actor)?;
    let resource = registry::resource(&input.resource, &actor, false)?;
    let headers = registry::fields(&input.fields, resource)?;
    let _slot = state
        .data_tool_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let result = tokio::task::spawn_blocking(move || {
        let _slot = _slot;
        exports::download(
            input.format,
            &input.resource,
            formats::Table {
                headers,
                rows: Vec::new(),
            },
        )
    })
    .await
    .map_err(|_| AppError::Internal)??;
    Ok(ApiResponse::success(result))
}
async fn inspect(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<ApiResponse<Inspection>, AppError> {
    actor.require("exports.write")?;
    let input = request::json::<InspectInput>(body, &state).await?;
    registry::resource(&input.resource, &Actor::new(&actor)?, true)?;
    let slot = state
        .data_tool_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        super::inspection::inspect(input)
    })
    .await
    .map_err(|_| AppError::Internal)??;
    Ok(ApiResponse::success(result))
}
async fn export(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<ApiResponse<exports::Download>, AppError> {
    actor.require("exports.write")?;
    let input = request::json::<ExportInput>(body, &state).await?;
    let _slot = state
        .data_tool_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let table = exports::export(state.require_database()?, &Actor::new(&actor)?, &input).await?;
    let result = tokio::task::spawn_blocking(move || {
        let _slot = _slot;
        exports::download(input.format, &input.resource, table)
    })
    .await
    .map_err(|_| AppError::Internal)??;
    Ok(ApiResponse::success(result))
}
async fn preview(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ImportView>), AppError> {
    actor.require("exports.write")?;
    let actor = Actor::new(&actor)?;
    let input = request::json::<PreviewInput>(body, &state).await?;
    registry::resource(&input.resource, &actor, true)?;
    let _slot = state
        .data_tool_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let (input, fingerprint, rows) = tokio::task::spawn_blocking(move || {
        let _slot = _slot;
        let (fingerprint, rows) = imports::prepare(&input)?;
        Ok::<_, AppError>((input, fingerprint, rows))
    })
    .await
    .map_err(|_| AppError::Internal)??;
    let view = imports::create(state.require_database()?, actor, input, fingerprint, rows).await?;
    Ok((StatusCode::CREATED, ApiResponse::success(view)))
}
async fn read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ImportView>, AppError> {
    actor.require("exports.read")?;
    Ok(ApiResponse::success(
        imports::get(
            state.require_database()?,
            &Actor::new(&actor)?,
            validation::object_id(&id)?,
        )
        .await?
        .view()?,
    ))
}
async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<super::history::HistoryQuery>, QueryRejection>,
) -> Result<crate::common::pagination::Page<ImportSummary>, AppError> {
    actor.require("exports.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("导入历史查询参数无效".into()))?;
    super::history::list(state.require_database()?, &Actor::new(&actor)?, input).await
}
async fn apply(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<ImportView>, AppError> {
    actor.require("exports.write")?;
    let input = request::json::<ApplyInput>(body, &state).await?;
    let _slot = state
        .data_tool_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    Ok(ApiResponse::success(
        imports::apply(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
