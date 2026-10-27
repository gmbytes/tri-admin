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
    get "/api/v1/workflows/templates"=>templates,"workflows.templates.list","workflows","内置 Prompt 工作流",Permission,&["workflows.read"];
    get "/api/v1/workflows/plans"=>plans,"workflows.plans.list","workflows","工作流定义列表",Permission,&["workflows.read"];
    post "/api/v1/workflows/plans"=>create,"workflows.plans.create","workflows","保存工作流定义",Permission,&["workflows.read","workflows.write"];
    get "/api/v1/workflows/plans/by-request/{requestId}"=>by_request,"workflows.plans.byRequest","workflows","恢复本人工作流定义请求",Permission,&["workflows.read"];
    get "/api/v1/workflows/plans/{id}"=>get,"workflows.plans.get","workflows","工作流定义元数据",Permission,&["workflows.read"];
    get "/api/v1/workflows/plans/{id}/definition"=>definition,"workflows.plans.definition","workflows","本人私有工作流步骤",Permission,&["workflows.read"];
    get "/api/v1/workflows/plans/{id}/revisions/{version}"=>revision,"workflows.plans.revision","workflows","本人固定定义修订",Permission,&["workflows.read"];
    patch "/api/v1/workflows/plans/{id}"=>patch,"workflows.plans.update","workflows","修改工作流定义",Permission,&["workflows.read","workflows.write"];
    delete "/api/v1/workflows/plans/{id}"=>delete,"workflows.plans.delete","workflows","删除工作流定义",Permission,&["workflows.read","workflows.write"];
    post "/api/v1/workflows/analyses"=>analyze,"workflows.analyses.create","workflows","提交结构化需求分析",Permission,&["ai.invoke","ai.read","workflows.invoke","workflows.read"];
    get "/api/v1/workflows/runs"=>runs,"workflows.runs.list","workflows","工作流任务列表",Permission,&["workflows.read"];
    post "/api/v1/workflows/runs"=>start,"workflows.runs.create","workflows","执行固定版本工作流",Permission,&["ai.invoke","ai.read","workflows.invoke","workflows.read"];
    get "/api/v1/workflows/runs/by-request/{requestId}"=>run_request,"workflows.runs.byRequest","workflows","按本人 UUID 恢复工作流",Permission,&["workflows.read"];
    get "/api/v1/workflows/runs/{id}"=>run,"workflows.runs.get","workflows","工作流任务元数据",Permission,&["workflows.read"];
    get "/api/v1/workflows/runs/{id}/content"=>content,"workflows.runs.content","workflows","原操作者读取私有步骤及结果",Permission,&["ai.invoke","ai.read","workflows.read"];
    post "/api/v1/workflows/runs/{id}/cancel"=>cancel,"workflows.runs.cancel","workflows","请求停止本人工作流",Permission,&["ai.invoke","ai.read","workflows.invoke","workflows.read"];
    post "/api/v1/workflows/runs/{id}/exports"=>export,"workflows.exports.create","workflows","创建本人私有 Markdown 产物",Permission,&["ai.invoke","ai.read","workflows.read"];
    get "/api/v1/workflows/exports/by-request/{requestId}"=>export_request,"workflows.exports.byRequest","workflows","恢复本人 Markdown 导出",Permission,&["ai.invoke","ai.read","workflows.read"];
    get "/api/v1/workflows/exports/{id}"=>export_get,"workflows.exports.get","workflows","本人导出清单",Permission,&["ai.invoke","ai.read","workflows.read"];
    get "/api/v1/workflows/exports/{id}/content"=>export_download,"workflows.exports.content","workflows","下载本人 Markdown 产物",Permission,&["ai.invoke","ai.read","workflows.read"];
    get "/api/v1/workflows/sessions"=>sessions,"workflows.sessions.list","workflows","会话历史列表",Permission,&["workflows.read"];
    post "/api/v1/workflows/sessions"=>session_create,"workflows.sessions.create","workflows","保存本人会话草稿",Permission,&["workflows.read","workflows.write"];
    get "/api/v1/workflows/sessions/by-request/{requestId}"=>session_request,"workflows.sessions.byRequest","workflows","恢复本人会话请求",Permission,&["workflows.read"];
    get "/api/v1/workflows/sessions/{id}"=>session_get,"workflows.sessions.get","workflows","会话元数据",Permission,&["workflows.read"];
    get "/api/v1/workflows/sessions/{id}/content"=>session_content,"workflows.sessions.content","workflows","本人私有草稿与选择",Permission,&["workflows.read"];
    patch "/api/v1/workflows/sessions/{id}"=>session_patch,"workflows.sessions.update","workflows","保存本人当前会话状态",Permission,&["workflows.read","workflows.write"];
    delete "/api/v1/workflows/sessions/{id}"=>session_delete,"workflows.sessions.delete","workflows","删除本人会话历史",Permission,&["workflows.read","workflows.write"];
    get "/api/v1/workflows/sessions/{id}/messages"=>messages,"workflows.messages.list","workflows","本人会话消息分支",Permission,&["workflows.read"];
    post "/api/v1/workflows/sessions/{id}/messages"=>message_add,"workflows.messages.create","workflows","引用固定运行并创建历史分支",Permission,&["workflows.read","workflows.write"];
    get "/api/v1/workflows/messages/{id}/content"=>message_content,"workflows.messages.content","workflows","本人消息的原始运行内容",Permission,&["ai.invoke","ai.read","workflows.read"];
    get "/api/v1/workflows/messages/by-request/{requestId}"=>message_request,"workflows.messages.byRequest","workflows","恢复本人消息引用请求",Permission,&["workflows.read"];
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
        .map_err(|_| AppError::Validation("工作流查询无效".into()))
}
fn write(context: &AuthContext) -> Result<(), AppError> {
    context.require("workflows.read")?;
    context.require("workflows.write")
}
async fn templates(context: AuthContext) -> Result<ApiResponse<Vec<Template>>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(service::templates()))
}
async fn plans(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<PlanView>, AppError> {
    context.require("workflows.read")?;
    service::plans(&state, context, query(input)?).await
}
async fn create(
    State(state): State<AppState>,
    context: AuthContext,
    request: Request,
) -> Result<(StatusCode, ApiResponse<PlanView>), AppError> {
    write(&context)?;
    let input = request::json::<CreatePlan>(request, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(service::create(&state, context, input).await?),
    ))
}
async fn get(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<PlanView>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        service::get(&state, context, Some(validation::object_id(&id)?), None).await?,
    ))
}
async fn by_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<PlanView>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        service::get(&state, context, None, Some(id)).await?,
    ))
}
async fn definition(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<DefinitionView>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        service::definition(&state, context, validation::object_id(&id)?, None).await?,
    ))
}
async fn revision(
    State(state): State<AppState>,
    context: AuthContext,
    Path((id, version)): Path<(String, String)>,
) -> Result<ApiResponse<DefinitionView>, AppError> {
    context.require("workflows.read")?;
    let version = version
        .parse::<i64>()
        .map_err(|_| AppError::Validation("版本无效".into()))?;
    Ok(ApiResponse::success(
        service::definition(&state, context, validation::object_id(&id)?, Some(version)).await?,
    ))
}
async fn patch(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    request: Request,
) -> Result<ApiResponse<PlanView>, AppError> {
    write(&context)?;
    let input = request::json::<PatchPlan>(request, &state).await?;
    Ok(ApiResponse::success(
        service::patch(&state, context, validation::object_id(&id)?, input).await?,
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
    context.require("workflows.read")?;
    context.require("workflows.invoke")?;
    context.require("ai.read")?;
    context.require("ai.invoke")
}
async fn start(
    State(state): State<AppState>,
    context: AuthContext,
    request: Request,
) -> Result<(StatusCode, ApiResponse<RunView>), AppError> {
    invoke(&context)?;
    let input = request::json::<Start>(request, &state).await?;
    Ok((
        StatusCode::ACCEPTED,
        ApiResponse::success(super::runs::start(&state, context, input).await?),
    ))
}
async fn analyze(
    State(state): State<AppState>,
    context: AuthContext,
    request: Request,
) -> Result<(StatusCode, ApiResponse<RunView>), AppError> {
    invoke(&context)?;
    let input = request::json::<Analyze>(request, &state).await?;
    Ok((
        StatusCode::ACCEPTED,
        ApiResponse::success(super::runs::analyze(&state, context, input).await?),
    ))
}
async fn runs(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<RunView>, AppError> {
    context.require("workflows.read")?;
    super::runs::list(&state, context, query(input)?).await
}
async fn run(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<RunView>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        super::runs::get(&state, context, Some(validation::object_id(&id)?), None).await?,
    ))
}
async fn run_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<RunView>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        super::runs::get(&state, context, None, Some(id)).await?,
    ))
}
async fn content(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<RunContent>, AppError> {
    context.require("workflows.read")?;
    context.require("ai.read")?;
    context.require("ai.invoke")?;
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
fn private_read(context: &AuthContext) -> Result<(), AppError> {
    context.require("workflows.read")?;
    context.require("ai.read")?;
    context.require("ai.invoke")
}
async fn export(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    request: Request,
) -> Result<(StatusCode, ApiResponse<ExportView>), AppError> {
    private_read(&context)?;
    let input = request::json::<ExportInput>(request, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            super::export::create(&state, context, validation::object_id(&id)?, input).await?,
        ),
    ))
}
async fn export_get(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ExportView>, AppError> {
    private_read(&context)?;
    Ok(ApiResponse::success(
        super::export::get(&state, context, Some(validation::object_id(&id)?), None).await?,
    ))
}
async fn export_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ExportView>, AppError> {
    private_read(&context)?;
    Ok(ApiResponse::success(
        super::export::get(&state, context, None, Some(id)).await?,
    ))
}
async fn export_download(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    use axum::response::IntoResponse;
    private_read(&context)?;
    let artifact = super::export::download(&state, context, validation::object_id(&id)?).await?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                "text/markdown; charset=utf-8".to_owned(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", artifact.file_name),
            ),
            (header::CONTENT_LENGTH, artifact.bytes.len().to_string()),
            (header::ETAG, format!("\"{}\"", artifact.sha256)),
        ],
        artifact.bytes,
    )
        .into_response())
}
async fn sessions(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<SessionView>, AppError> {
    context.require("workflows.read")?;
    super::sessions::list(&state, context, query(input)?).await
}
async fn session_create(
    State(state): State<AppState>,
    context: AuthContext,
    request: Request,
) -> Result<(StatusCode, ApiResponse<SessionView>), AppError> {
    write(&context)?;
    let input = request::json::<CreateSession>(request, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(super::sessions::create(&state, context, input).await?),
    ))
}
async fn session_get(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<SessionView>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        super::sessions::get(&state, context, Some(validation::object_id(&id)?), None).await?,
    ))
}
async fn session_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<SessionView>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        super::sessions::get(&state, context, None, Some(id)).await?,
    ))
}
async fn session_content(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<SessionContent>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        super::sessions::content(&state, context, validation::object_id(&id)?).await?,
    ))
}
async fn session_patch(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    request: Request,
) -> Result<ApiResponse<SessionView>, AppError> {
    write(&context)?;
    let input = request::json::<PatchSession>(request, &state).await?;
    Ok(ApiResponse::success(
        super::sessions::patch(&state, context, validation::object_id(&id)?, input).await?,
    ))
}
async fn session_delete(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<Delete>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    write(&context)?;
    super::sessions::delete(
        &state,
        context,
        validation::object_id(&id)?,
        query(input)?.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn messages(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<MessageView>, AppError> {
    context.require("workflows.read")?;
    super::sessions::messages(&state, context, validation::object_id(&id)?, query(input)?).await
}
async fn message_add(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    request: Request,
) -> Result<(StatusCode, ApiResponse<MessageView>), AppError> {
    write(&context)?;
    let input = request::json::<AddMessage>(request, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            super::sessions::add(&state, context, validation::object_id(&id)?, input).await?,
        ),
    ))
}
async fn message_content(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<MessageContent>, AppError> {
    private_read(&context)?;
    Ok(ApiResponse::success(
        super::sessions::message_content(&state, context, validation::object_id(&id)?).await?,
    ))
}
async fn message_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(request): Path<String>,
) -> Result<ApiResponse<MessageView>, AppError> {
    context.require("workflows.read")?;
    Ok(ApiResponse::success(
        super::sessions::message_request(&state, context, request).await?,
    ))
}
