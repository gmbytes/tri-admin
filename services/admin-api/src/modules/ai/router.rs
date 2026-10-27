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
    response::Response,
};
crate::api_routes! {
    prefix "";
    get "/api/v1/ai/providers"=>providers,"ai.providers.list","ai","AI 上游列表",Permission,&["ai.read"];
    get "/api/v1/ai/models"=>models,"ai.models.list","ai","批准的 AI 模型目录",Permission,&["ai.read"];
    post "/api/v1/ai/providers"=>create,"ai.providers.create","ai","批准 AI 上游",Permission,&["ai.read","ai.write","settings.write"],"全部数据范围；不发出模型请求";
    get "/api/v1/ai/providers/{id}"=>get,"ai.providers.get","ai","AI 上游资料",Permission,&["ai.read"];
    patch "/api/v1/ai/providers/{id}"=>patch,"ai.providers.update","ai","修改 AI 上游",Permission,&["ai.read","ai.write","settings.write"];
    delete "/api/v1/ai/providers/{id}"=>delete,"ai.providers.delete","ai","删除 AI 上游",Permission,&["ai.read","ai.write","settings.write"];
    get "/api/v1/ai/runs"=>runs,"ai.runs.list","ai","AI 运行元数据列表",Permission,&["ai.read"];
    post "/api/v1/ai/runs"=>create_run,"ai.runs.create","ai","提交 AI 生成意图",Permission,&["ai.invoke","ai.read"],"原 UUID 只生成一次；输入加密保存";
    get "/api/v1/ai/runs/by-request/{requestId}"=>run_request,"ai.runs.byRequest","ai","按本人 UUID 恢复 AI 运行",Permission,&["ai.read"];
    get "/api/v1/ai/runs/{id}"=>run,"ai.runs.get","ai","AI 运行元数据",Permission,&["ai.read"];
    get "/api/v1/ai/runs/{id}/content"=>content,"ai.runs.content","ai","原操作者读取私有 AI 输入和输出",Permission,&["ai.invoke","ai.read"];
    post "/api/v1/ai/runs/{id}/cancel"=>cancel,"ai.runs.cancel","ai","请求取消本人 AI 运行",Permission,&["ai.invoke","ai.read"];
}
pub fn configured_routes() -> axum::Router<AppState> {
    routes()
        .layer(axum::extract::DefaultBodyLimit::max(512 * 1024))
        .layer(axum::middleware::from_fn(private))
}
async fn private(request: Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
fn query<T>(value: Result<Query<T>, QueryRejection>) -> Result<T, AppError> {
    value
        .map(|Query(value)| value)
        .map_err(|_| AppError::Validation("AI 查询无效".into()))
}
fn write(context: &AuthContext) -> Result<(), AppError> {
    context.require("ai.read")?;
    context.require("ai.write")?;
    context.require("settings.write")
}
async fn providers(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<ProviderView>, AppError> {
    context.require("ai.read")?;
    service::list(&state, context, query(input)?).await
}
async fn models(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<ApprovedModel>, AppError> {
    context.require("ai.read")?;
    service::models(&state, context, query(input)?).await
}
async fn get(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ProviderView>, AppError> {
    context.require("ai.read")?;
    Ok(ApiResponse::success(
        service::get(&state, context, validation::object_id(&id)?).await?,
    ))
}
async fn create(
    State(state): State<AppState>,
    context: AuthContext,
    request: Request,
) -> Result<(StatusCode, ApiResponse<ProviderView>), AppError> {
    write(&context)?;
    let input = request::json::<ProviderInput>(request, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(service::save(&state, context, None, input).await?),
    ))
}
async fn patch(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    request: Request,
) -> Result<ApiResponse<ProviderView>, AppError> {
    write(&context)?;
    let input = request::json::<ProviderPatch>(request, &state).await?;
    let version = input.version;
    Ok(ApiResponse::success(
        service::save(
            &state,
            context,
            Some((validation::object_id(&id)?, version)),
            input.input(),
        )
        .await?,
    ))
}
async fn delete(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<Delete>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    write(&context)?;
    service::delete(
        &state,
        context,
        validation::object_id(&id)?,
        query(input)?.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
fn invoke(context: &AuthContext) -> Result<(), AppError> {
    context.require("ai.read")?;
    context.require("ai.invoke")
}
async fn runs(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<RunView>, AppError> {
    context.require("ai.read")?;
    super::runs::list(&state, context, query(input)?).await
}
async fn create_run(
    State(state): State<AppState>,
    context: AuthContext,
    request: Request,
) -> Result<(StatusCode, ApiResponse<RunView>), AppError> {
    invoke(&context)?;
    let input = request::json::<RunInput>(request, &state).await?;
    Ok((
        StatusCode::ACCEPTED,
        ApiResponse::success(super::runs::create(&state, context, input).await?),
    ))
}
async fn run(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<RunView>, AppError> {
    context.require("ai.read")?;
    Ok(ApiResponse::success(
        super::runs::get(&state, context, Some(validation::object_id(&id)?), None).await?,
    ))
}
async fn run_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<RunView>, AppError> {
    context.require("ai.read")?;
    Ok(ApiResponse::success(
        super::runs::get(&state, context, None, Some(id)).await?,
    ))
}
async fn content(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<Content>, AppError> {
    invoke(&context)?;
    Ok(ApiResponse::success(
        super::runs::content(&state, context, validation::object_id(&id)?).await?,
    ))
}
async fn cancel(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    request: Request,
) -> Result<(StatusCode, ApiResponse<RunView>), AppError> {
    invoke(&context)?;
    let input = request::json::<Cancel>(request, &state).await?;
    Ok((
        StatusCode::ACCEPTED,
        ApiResponse::success(
            super::runs::cancel(&state, context, validation::object_id(&id)?, input).await?,
        ),
    ))
}
