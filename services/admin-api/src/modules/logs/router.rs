use super::{
    dto::{CleanupInput, LogInput},
    models::{CleanupResult, LogView},
    service,
};
use crate::{
    common::{error::AppError, pagination::Page, request, response::ApiResponse},
    modules::{auth::AuthContext, rbac::shared::Actor},
    state::AppState,
};
use axum::extract::{Query, Request, State, rejection::QueryRejection};
crate::api_routes! {
 prefix "";
 get "/api/v1/logs" => list, "logs.list", "audit", "查询登录操作与错误日志", Permission, &["audit.read"];
 post "/api/v1/logs/cleanup" => cleanup, "logs.cleanup", "audit", "分批清理历史日志", Permission, &["audit.delete","audit.read"], "只清理至少24小时前记录，每批最多1000条；requestId保证幂等";
}
async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<LogInput>, QueryRejection>,
) -> Result<Page<LogView>, AppError> {
    actor.require("audit.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("日志查询参数无效".into()))?;
    service::list(state.require_database()?, input).await
}
async fn cleanup(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<ApiResponse<CleanupResult>, AppError> {
    actor.require("audit.read")?;
    actor.require("audit.delete")?;
    let input = request::json::<CleanupInput>(body, &state).await?;
    Ok(ApiResponse::success(
        service::cleanup(state.require_database()?, Actor::new(&actor)?, input).await?,
    ))
}
