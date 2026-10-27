use super::business;
use crate::{
    common::{
        error::{AppError, ResponseError},
        observability,
    },
    modules::auth::AuthContext,
    state::AppState,
};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    response::Response,
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use serde_json::{Value, json};
use std::{borrow::Cow, sync::Arc};
use tower::ServiceExt;

// A new stateless service is bound to this HTTP request's authenticated identity.
// No session, SDK handshake, or tool-supplied argument can replace that identity.
#[derive(Clone)]
struct BusinessServer {
    state: AppState,
    context: AuthContext,
    trace: observability::CapturedRequest,
}
impl ServerHandler for BusinessServer {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![ProtocolVersion::V_2026_07_28])
    }
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request
            .as_ref()
            .and_then(|value| value.cursor.as_ref())
            .is_some()
        {
            return Err(ErrorData::invalid_params("不支持该工具游标", None));
        }
        let tools = self
            .trace
            .scope(crate::modules::extensions::contributions::catalog(
                &self.state,
                &self.context,
            ))
            .await
            .map_err(|_| ErrorData::internal_error("工具目录暂不可用", None))?;
        Ok(ListToolsResult::with_all_items(tools))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let plugin = request.name.starts_with("plugin.");
        self.trace
            .scope(async {
                if plugin {
                    crate::modules::extensions::contributions::call(
                        &self.state,
                        &self.context,
                        request,
                    )
                    .await
                } else {
                    business::call(&self.state, &self.context, request).await
                }
            })
            .await
            .map(Into::into)
    }
}
pub async fn handle(
    state: AppState,
    context: AuthContext,
    request: Request,
) -> Result<Response, AppError> {
    if request.headers().get_all("origin").iter().count() > 1
        || request
            .headers()
            .get("origin")
            .is_some_and(|origin| !state.config.cors_origins.contains(origin))
    {
        return Err(AppError::Forbidden);
    }
    if request
        .headers()
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok())
        != Some("2026-07-28")
    {
        return Ok(protocol_error(
            axum::http::StatusCode::BAD_REQUEST,
            -32022,
            "仅支持 MCP 2026-07-28",
            Value::Null,
        ));
    }
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 128 * 1024)
        .await
        .map_err(|_| AppError::PayloadTooLarge)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| AppError::BadRequest)?;
    if !value.is_object()
        || matches!(
            value["method"].as_str(),
            Some("initialize" | "notifications/initialized")
        )
    {
        return Ok(protocol_error(
            axum::http::StatusCode::BAD_REQUEST,
            -32600,
            "此端点不接受旧握手或批量消息",
            Value::Null,
        ));
    }
    let service_state = state.clone();
    let trace = observability::capture_request();
    let service = StreamableHttpService::new(
        move || {
            Ok(BusinessServer {
                state: service_state.clone(),
                context: context.clone(),
                trace: trace.clone(),
            })
        },
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(state.config.mcp_allowed_hosts.clone())
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None)
            .with_stateless_protocol_metadata_required(true)
            .with_max_request_body_bytes(128 * 1024)
            .with_cancellation_token(state.mcp.shutdown.child_token()),
    );
    let response = match service
        .oneshot(Request::from_parts(parts, Body::from(bytes)))
        .await
    {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let (parts, body) = response.into_parts();
    if parts.status.is_client_error() || parts.status.is_server_error() {
        // Keep protocol errors as JSON-RPC while excluding SDK body/header details.
        let bytes = to_bytes(Body::new(body), 16 * 1024)
            .await
            .unwrap_or_default();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let code = value["error"]["code"]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .unwrap_or(if parts.status == axum::http::StatusCode::NOT_FOUND {
                -32601
            } else {
                -32600
            });
        let id = match &value["id"] {
            Value::String(v) if v.len() <= 128 => Value::String(v.clone()),
            Value::Number(v) => Value::Number(v.clone()),
            _ => Value::Null,
        };
        return Ok(protocol_error(
            parts.status,
            code,
            "MCP 请求被拒绝，请检查协议元数据和方法",
            id,
        ));
    }
    Ok(Response::from_parts(parts, Body::new(body)))
}
fn protocol_error(
    status: axum::http::StatusCode,
    code: i32,
    message: &'static str,
    id: Value,
) -> Response {
    use axum::response::IntoResponse;
    let mut error = json!({"code":code,"message":message});
    if code == -32022 {
        error["data"] = json!({"supported":["2026-07-28"]});
    }
    let mut response = (
        status,
        axum::Json(json!({"jsonrpc":"2.0","id":id,"error":error})),
    )
        .into_response();
    response.extensions_mut().insert(ResponseError {
        code: "MCP_PROTOCOL_ERROR",
    });
    response
}
