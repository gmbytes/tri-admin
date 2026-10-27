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
 get "/api/v1/mcp/capabilities"=>capabilities,"mcp.capabilities.list","mcp","部署批准的 MCP 程序能力",Permission,&["mcp.read"];
 get "/api/v1/mcp/connections"=>connections,"mcp.connections.list","mcp","MCP 连接列表",Permission,&["mcp.read"];
 post "/api/v1/mcp/connections"=>create,"mcp.connections.create","mcp","创建 MCP 连接",Permission,&["mcp.read","mcp.write"];
 get "/api/v1/mcp/connections/by-request/{requestId}"=>connection_request,"mcp.connections.byRequest","mcp","按本人请求查询连接",Permission,&["mcp.read"];
 get "/api/v1/mcp/connections/{id}"=>connection,"mcp.connections.get","mcp","MCP 连接资料",Permission,&["mcp.read"];
 patch "/api/v1/mcp/connections/{id}"=>patch,"mcp.connections.update","mcp","修改 MCP 连接",Permission,&["mcp.read","mcp.write"];
 delete "/api/v1/mcp/connections/{id}"=>delete,"mcp.connections.delete","mcp","删除 MCP 连接",Permission,&["mcp.read","mcp.write"];
 get "/api/v1/mcp/connections/{id}/tools"=>tools,"mcp.tools.list","mcp","MCP 批准工具清单",Permission,&["mcp.read"];
 get "/api/v1/mcp/connections/{id}/actions"=>actions,"mcp.actions.list","mcp","MCP 操作历史",Permission,&["mcp.read"];
 post "/api/v1/mcp/connections/{id}/actions"=>action,"mcp.actions.create","mcp","MCP 启动停止重启",Permission,&["mcp.read","mcp.write"],"先持久记录意图；同一 UUID 不重复执行";
 post "/api/v1/mcp/connections/{id}/calls"=>call,"mcp.calls.create","mcp","调用批准的 MCP 工具",Permission,&["mcp.invoke","mcp.read"],"不确定效果不会自动重发";
 get "/api/v1/mcp/actions/by-request/{requestId}"=>action_request,"mcp.actions.byRequest","mcp","按本人 UUID 查询原操作",Permission,&["mcp.read"];
 get "/api/v1/mcp/actions/{id}"=>get_action,"mcp.actions.get","mcp","MCP 操作回执",Permission,&["mcp.read"];
 get "/api/v1/mcp/actions/{id}/result"=>result,"mcp.actions.result","mcp","原调用者读取加密结果",Permission,&["mcp.invoke","mcp.read"];
 post "/api/v1/mcp/actions/{id}/cancel"=>cancel,"mcp.actions.cancel","mcp","请求取消 MCP 调用",Permission,&["mcp.invoke","mcp.read"];
 post "/api/v1/mcp/connections/{id}/confirm-orphan"=>confirm_orphan,"mcp.connections.confirmOrphan","mcp","交互确认遗留进程已清理",Permission,&["mcp.read","mcp.write"],"仅交互会话与全部数据范围；不根据旧 PID 终止任何进程";
}
pub fn configured_routes() -> axum::Router<AppState> {
    routes()
        .layer(axum::extract::DefaultBodyLimit::max(128 * 1024))
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
fn read(context: &AuthContext) -> Result<(), AppError> {
    context.require("mcp.read")
}
fn write(context: &AuthContext) -> Result<(), AppError> {
    read(context)?;
    context.require("mcp.write")
}
fn invoke(context: &AuthContext) -> Result<(), AppError> {
    read(context)?;
    context.require("mcp.invoke")
}
fn query<T>(value: Result<Query<T>, QueryRejection>) -> Result<T, AppError> {
    value
        .map(|Query(value)| value)
        .map_err(|_| AppError::Validation("MCP 查询无效".into()))
}
async fn capabilities(
    State(state): State<AppState>,
    context: AuthContext,
) -> Result<ApiResponse<Vec<super::capabilities::CapabilityView>>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::capabilities(&state, &context).await?,
    ))
}
async fn connections(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<ConnectionView>, AppError> {
    read(&context)?;
    service::connections(&state, &context, query(input)?).await
}
async fn create(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ConnectionView>), AppError> {
    write(&context)?;
    let input = request::json::<Create>(body, &state).await?;
    let (row, created) = service::create(&state, &context, input).await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
async fn connection_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ConnectionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::by_request(&state, &context, &id).await?,
    ))
}
async fn connection(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ConnectionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::get(&state, &context, validation::object_id(&id)?).await?,
    ))
}
async fn patch(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<ConnectionView>, AppError> {
    write(&context)?;
    let input = request::json::<Patch>(body, &state).await?;
    Ok(ApiResponse::success(
        service::patch(&state, &context, validation::object_id(&id)?, input).await?,
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
        &context,
        validation::object_id(&id)?,
        query(input)?.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn tools(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<Vec<serde_json::Value>>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::tools(&state, &context, validation::object_id(&id)?).await?,
    ))
}
async fn actions(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<ActionView>, AppError> {
    read(&context)?;
    service::actions(&state, &context, validation::object_id(&id)?, query(input)?).await
}
async fn action(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ActionView>), AppError> {
    write(&context)?;
    let input = request::json::<ActionInput>(body, &state).await?;
    let (row, created) =
        service::submit_action(&state, &context, validation::object_id(&id)?, input).await?;
    Ok((
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
async fn call(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ActionView>), AppError> {
    invoke(&context)?;
    let input = request::json::<Call>(body, &state).await?;
    let (row, created) =
        service::submit_call(&state, &context, validation::object_id(&id)?, input).await?;
    Ok((
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
async fn action_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ActionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::action_by_request(&state, &context, &id).await?,
    ))
}
async fn get_action(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ActionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::get_action(&state, &context, validation::object_id(&id)?).await?,
    ))
}
async fn result(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ResultView>, AppError> {
    invoke(&context)?;
    Ok(ApiResponse::success(
        service::result(&state, &context, validation::object_id(&id)?).await?,
    ))
}
async fn cancel(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ActionView>), AppError> {
    invoke(&context)?;
    let input = request::json::<CancelInput>(body, &state).await?;
    let (row, created) =
        service::cancel_call(&state, &context, validation::object_id(&id)?, input).await?;
    Ok((
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
async fn confirm_orphan(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<ActionView>, AppError> {
    write(&context)?;
    context.require_session()?;
    let input = request::json::<OrphanConfirm>(body, &state).await?;
    Ok(ApiResponse::success(
        service::confirm_orphan(&state, &context, validation::object_id(&id)?, input).await?,
    ))
}
