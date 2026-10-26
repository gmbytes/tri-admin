use super::{dto::*, models::*, service};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        request,
        response::ApiResponse,
        validation,
    },
    modules::auth::AuthContext,
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
crate::api_routes! {
 prefix "";
 get "/api/v1/codegen/definitions"=>definitions,"codegen.definitions.list","codegen","代码生成定义列表",Permission,&["codegen.read"];
 post "/api/v1/codegen/definitions"=>create,"codegen.definitions.create","codegen","创建代码生成定义",Permission,&["codegen.read","codegen.write"];
 get "/api/v1/codegen/definitions/by-request/{requestId}"=>definition_request,"codegen.definitions.byRequest","codegen","按请求查询本人定义",Permission,&["codegen.read"];
 get "/api/v1/codegen/definitions/{id}"=>definition,"codegen.definitions.get","codegen","代码生成定义资料",Permission,&["codegen.read"];
 patch "/api/v1/codegen/definitions/{id}"=>update,"codegen.definitions.update","codegen","修改代码生成定义",Permission,&["codegen.read","codegen.write"];
 delete "/api/v1/codegen/definitions/{id}"=>delete,"codegen.definitions.delete","codegen","删除代码生成定义",Permission,&["codegen.read","codegen.write"];
 get "/api/v1/codegen/generations"=>generations,"codegen.generations.list","codegen","代码生成历史",Permission,&["codegen.read"];
 post "/api/v1/codegen/generations"=>generate,"codegen.generations.create","codegen","生成独立源码工程",Permission,&["codegen.read","codegen.write"],"固定 Rust/MongoDB/Vue 源码，服务端不执行或部署生成代码";
 get "/api/v1/codegen/generations/by-request/{requestId}"=>generation_request,"codegen.generations.byRequest","codegen","按请求查询本人生成结果",Permission,&["codegen.read"];
 get "/api/v1/codegen/generations/{id}"=>generation,"codegen.generations.get","codegen","代码生成结果资料",Permission,&["codegen.read"];
 get "/api/v1/codegen/generations/{id}/preview"=>preview,"codegen.generations.preview","codegen","预览原生成文件",Permission,&["codegen.read"];
 get "/api/v1/codegen/generations/{id}/download"=>download,"codegen.generations.download","codegen","下载原生成工程",Permission,&["codegen.read"];
}
pub fn configured_routes() -> axum::Router<AppState> {
    routes()
        .layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::from_fn(private))
}
async fn private(request: Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
fn read(context: &AuthContext) -> Result<(), AppError> {
    context.require("codegen.read")
}
fn write(context: &AuthContext) -> Result<(), AppError> {
    read(context)?;
    context.require("codegen.write")
}
async fn definitions(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<DefinitionView>, AppError> {
    read(&context)?;
    let Query(input) = input.map_err(|_| AppError::Validation("定义查询无效".into()))?;
    service::definitions(state.require_database()?, &context, input).await
}
async fn create(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<DefinitionView>), AppError> {
    write(&context)?;
    let input = request::json::<CreateDefinition>(body, &state).await?;
    let (row, created) =
        service::create_definition(state.require_database()?, &context, input).await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
async fn definition(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<DefinitionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::definition(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn definition_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<DefinitionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::definition_by_request(state.require_database()?, &context, &id).await?,
    ))
}
async fn update(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<DefinitionView>, AppError> {
    write(&context)?;
    let input = request::json::<UpdateDefinition>(body, &state).await?;
    Ok(ApiResponse::success(
        service::update_definition(
            state.require_database()?,
            &context,
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
    input: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    write(&context)?;
    let Query(input) = input.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    service::delete_definition(
        state.require_database()?,
        &context,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn generations(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<HistoryQuery>, QueryRejection>,
) -> Result<Page<GenerationView>, AppError> {
    read(&context)?;
    let Query(input) = input.map_err(|_| AppError::Validation("生成历史查询无效".into()))?;
    service::generations(state.require_database()?, &context, input).await
}
async fn generate(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<GenerationView>), AppError> {
    write(&context)?;
    let input = request::json::<Generate>(body, &state).await?;
    let (row, created) = service::generate(state.require_database()?, &context, input).await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
async fn generation(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<GenerationView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::generation(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn generation_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<GenerationView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::generation_by_request(state.require_database()?, &context, &id).await?,
    ))
}
async fn preview(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<Preview>, QueryRejection>,
) -> Result<ApiResponse<SourceView>, AppError> {
    read(&context)?;
    let Query(input) = input.map_err(|_| AppError::Validation("预览参数无效".into()))?;
    Ok(ApiResponse::success(
        service::preview(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
            &input.path,
        )
        .await?,
    ))
}
async fn download(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    read(&context)?;
    let id = validation::object_id(&id)?;
    let (meta, bytes) = service::package(state.require_database()?, &context, id).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"codegen-{}.zip\"", id.to_hex()),
            ),
            (header::CONTENT_LENGTH, bytes.len().to_string()),
            (header::ETAG, format!("\"{}\"", meta.sha256)),
            (
                header::HeaderName::from_static("x-content-sha256"),
                meta.sha256,
            ),
        ],
        bytes,
    )
        .into_response())
}
