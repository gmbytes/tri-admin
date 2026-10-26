use super::{
    dto::{CreateToken, Empty},
    models::{IssueView, TokenView},
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
        open_access::usage::{self, CallView, CallsInput},
        rbac::shared::Actor,
    },
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::{HeaderValue, StatusCode, header::CACHE_CONTROL},
    middleware::Next,
    response::Response,
};
crate::api_routes! {
    prefix "";
    get "/api/v1/api-tokens"=>list,"apiTokens.list","apiTokens","我的 API Token",Permission,&["apiTokens.read"];
    post "/api/v1/api-tokens"=>create,"apiTokens.create","apiTokens","签发 API Token",Permission,&["apiTokens.write"],"仅交互会话；秘密仅首次响应展示";
    get "/api/v1/api-tokens/calls"=>calls,"apiTokens.calls","apiTokens","我的 Token 调用日志",Permission,&["apiTokens.read"];
    get "/api/v1/api-tokens/by-request/{requestId}"=>by_request,"apiTokens.byRequest","apiTokens","按请求查询 Token",Permission,&["apiTokens.read"];
    get "/api/v1/api-tokens/{id}"=>get,"apiTokens.get","apiTokens","Token 元数据",Permission,&["apiTokens.read"];
    post "/api/v1/api-tokens/{id}/revoke"=>revoke,"apiTokens.revoke","apiTokens","撤销 Token",Permission,&["apiTokens.write"],"仅交互会话";
}
pub fn configured_routes() -> axum::Router<AppState> {
    routes().layer(axum::middleware::from_fn(private))
}
async fn private(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}
async fn create(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<IssueView>), AppError> {
    context.require("apiTokens.write")?;
    context.require_session()?;
    let input = request::json::<CreateToken>(body, &state).await?;
    let (view, created) = service::create(state.require_database()?, &context, input).await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(view),
    ))
}
async fn list(
    State(state): State<AppState>,
    context: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<TokenView>, AppError> {
    context.require("apiTokens.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("Token 查询参数无效".into()))?;
    service::list(state.require_database()?, Actor::new(&context)?.id, input).await
}
async fn get(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<TokenView>, AppError> {
    context.require("apiTokens.read")?;
    Ok(ApiResponse::success(
        service::get(
            state.require_database()?,
            Actor::new(&context)?.id,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn by_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(request): Path<String>,
) -> Result<ApiResponse<TokenView>, AppError> {
    context.require("apiTokens.read")?;
    Ok(ApiResponse::success(
        service::by_request(
            state.require_database()?,
            Actor::new(&context)?.id,
            &request,
        )
        .await?,
    ))
}
async fn revoke(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<TokenView>, AppError> {
    context.require("apiTokens.write")?;
    context.require_session()?;
    let _ = request::json::<Empty>(body, &state).await?;
    Ok(ApiResponse::success(
        service::revoke(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn calls(
    State(state): State<AppState>,
    context: AuthContext,
    query: Result<Query<CallsInput>, QueryRejection>,
) -> Result<Page<CallView>, AppError> {
    context.require("apiTokens.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("调用日志查询无效".into()))?;
    usage::list(state.require_database()?, Actor::new(&context)?.id, input).await
}
