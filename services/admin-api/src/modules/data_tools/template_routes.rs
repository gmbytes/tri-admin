use super::{
    exports,
    formats::{self, Format},
    imports,
    models::{ExportInput, ImportView, PreviewInput, TemplateOrigin},
    query, registry,
    templates::{self, CopyInput, TemplateInput, TemplatePatch, TemplateView},
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
use std::collections::{BTreeMap, HashSet};
fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateQuery {
    #[serde(default = "first")]
    page: u64,
    #[serde(default = "size")]
    page_size: u64,
    search: Option<String>,
    sort: Option<String>,
    resource: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteInput {
    version: i64,
    format: Format,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportInput {
    version: i64,
    request_id: String,
    format: Format,
    content_base64: String,
    #[serde(default)]
    mapping: BTreeMap<String, String>,
}
pub async fn template_list(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<TemplateQuery>, QueryRejection>,
) -> Result<Page<TemplateView>, AppError> {
    actor.require("exports.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("模板查询参数无效".into()))?;
    templates::list(
        state.require_database()?,
        &Actor::new(&actor)?,
        PageInput {
            page: input.page,
            page_size: input.page_size,
            search: input.search,
            sort: input.sort,
        },
        input.resource,
    )
    .await
}
pub async fn template_create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<TemplateView>), AppError> {
    actor.require("exports.write")?;
    let input = request::json::<TemplateInput>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            templates::create(state.require_database()?, Actor::new(&actor)?, input).await?,
        ),
    ))
}
pub async fn template_get(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<TemplateView>, AppError> {
    actor.require("exports.read")?;
    Ok(ApiResponse::success(
        templates::get(
            state.require_database()?,
            &Actor::new(&actor)?,
            validation::object_id(&id)?,
        )
        .await?
        .view()?,
    ))
}
pub async fn template_patch(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<TemplateView>, AppError> {
    actor.require("exports.write")?;
    let input = request::json::<TemplatePatch>(body, &state).await?;
    Ok(ApiResponse::success(
        templates::patch(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
pub async fn template_delete(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    actor.require("exports.write")?;
    let Query(input) = query.map_err(|_| AppError::Validation("模板版本无效".into()))?;
    templates::delete(
        state.require_database()?,
        Actor::new(&actor)?,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn template_copy(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<TemplateView>), AppError> {
    actor.require("exports.write")?;
    let input = request::json::<CopyInput>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            templates::copy(
                state.require_database()?,
                Actor::new(&actor)?,
                validation::object_id(&id)?,
                input,
            )
            .await?,
        ),
    ))
}
pub async fn query_schema(
    actor: AuthContext,
    Path(resource): Path<String>,
) -> Result<ApiResponse<query::QuerySchema>, AppError> {
    actor.require("exports.read")?;
    registry::resource(&resource, &Actor::new(&actor)?, false)?;
    Ok(ApiResponse::success(query::schema(&resource)?))
}
pub async fn template_download(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<exports::Download>, AppError> {
    actor.require("exports.read")?;
    let input = request::json::<ExecuteInput>(body, &state).await?;
    let record = templates::active(
        state.require_database()?,
        &Actor::new(&actor)?,
        validation::object_id(&id)?,
        input.version,
        false,
    )
    .await?;
    let slot = state
        .data_tool_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        exports::download(
            input.format,
            &record.config.resource,
            formats::Table {
                headers: record
                    .config
                    .columns
                    .into_iter()
                    .map(|column| column.header)
                    .collect(),
                rows: Vec::new(),
            },
        )
    })
    .await
    .map_err(|_| AppError::Internal)??;
    Ok(ApiResponse::success(result))
}
pub async fn template_export(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<exports::Download>, AppError> {
    actor.require("exports.write")?;
    let input = request::json::<ExecuteInput>(body, &state).await?;
    let actor = Actor::new(&actor)?;
    let record = templates::active(
        state.require_database()?,
        &actor,
        validation::object_id(&id)?,
        input.version,
        false,
    )
    .await?;
    let slot = state
        .data_tool_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let request = ExportInput {
        resource: record.config.resource.clone(),
        format: input.format,
        fields: record
            .config
            .columns
            .iter()
            .map(|column| column.field.clone())
            .collect(),
        search: record.config.search.clone(),
        filters: record.config.filters.clone(),
        sort: record.config.sort.clone(),
    };
    let mut table = exports::export_limited(
        state.require_database()?,
        &actor,
        &request,
        record.config.max_rows as usize,
    )
    .await?;
    table.headers = record
        .config
        .columns
        .into_iter()
        .map(|column| column.header)
        .collect();
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        exports::download(input.format, &request.resource, table)
    })
    .await
    .map_err(|_| AppError::Internal)??;
    Ok(ApiResponse::success(result))
}
pub async fn template_import(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ImportView>), AppError> {
    actor.require("exports.write")?;
    let input = request::json::<ImportInput>(body, &state).await?;
    let actor = Actor::new(&actor)?;
    let record = templates::active(
        state.require_database()?,
        &actor,
        validation::object_id(&id)?,
        input.version,
        true,
    )
    .await?;
    let fields = record
        .config
        .columns
        .iter()
        .map(|column| column.field.as_str())
        .collect::<HashSet<_>>();
    if input
        .mapping
        .keys()
        .any(|field| !fields.contains(field.as_str()))
    {
        return Err(AppError::Validation("映射包含模板未允许的字段".into()));
    }
    let mapping = if input.mapping.is_empty() {
        record
            .config
            .columns
            .iter()
            .map(|column| (column.field.clone(), column.header.clone()))
            .collect()
    } else {
        input.mapping
    };
    let request = PreviewInput {
        request_id: input.request_id,
        resource: record.config.resource.clone(),
        format: input.format,
        content_base64: input.content_base64,
        mapping,
    };
    let origin = TemplateOrigin {
        id: record.id.to_hex(),
        code: record.config.code,
        name: record.config.name,
        version: record.version,
    };
    let slot = state
        .data_tool_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let (request, fingerprint, rows) = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let (fingerprint, rows) = imports::prepare(&request)?;
        Ok::<_, AppError>((request, fingerprint, rows))
    })
    .await
    .map_err(|_| AppError::Internal)??;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            imports::create(
                state.require_database()?,
                actor,
                request,
                fingerprint,
                rows,
                Some(origin),
            )
            .await?,
        ),
    ))
}
pub async fn import_by_request(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(request): Path<String>,
) -> Result<ApiResponse<ImportView>, AppError> {
    actor.require("exports.read")?;
    Ok(ApiResponse::success(
        imports::by_request(state.require_database()?, &Actor::new(&actor)?, &request)
            .await?
            .view()?,
    ))
}
