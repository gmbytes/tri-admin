//! Shared bounded RPC session. Process ownership and HTTP policy live in their transports.
use rmcp::{
    RoleClient,
    model::{
        CallToolRequest, CallToolRequestParams, ClientConfig, ClientRequest, ListToolsRequest,
        PaginatedRequestParams, ServerResult,
    },
    service::{
        ClientCacheConfig, Peer, PeerRequestOptions, RequestHandle, RunningService, ServiceError,
    },
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
pub(crate) struct SessionControl {
    pub stop: CancellationToken,
    pub fault: Arc<AtomicBool>,
}
impl SessionControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            stop: CancellationToken::new(),
            fault: Arc::new(AtomicBool::new(false)),
        })
    }
}
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("MCP 连接或运行限制无效")]
    InvalidInput,
    #[error("MCP 进程启动失败")]
    Spawn,
    #[error("MCP 新协议发现失败")]
    StartRejected,
    #[error("MCP 新协议发现超时")]
    StartTimeout,
    #[error("MCP 响应超过大小或数量限制")]
    Limit,
    #[error("MCP 调用在发送前取消")]
    CancelledBeforeSend,
    #[error("MCP 并发槽位已满，调用尚未发送")]
    BusyBeforeSend,
    #[error("MCP 调用已取消，远端效果未确认")]
    CancelledUnknown,
    #[error("MCP 调用超时，远端效果未确认")]
    TimeoutUnknown,
    #[error("MCP 连接中断，远端效果未确认")]
    TransportUnknown,
    #[error("MCP 远端返回错误，代码 {code}")]
    RemoteRejected { code: i32 },
    #[error("MCP 远端返回错误，效果未确认，代码 {code}")]
    RemoteUnknown { code: i32 },
    #[error("MCP 连接不可用")]
    Unavailable,
    #[error("MCP 进程退出尚未确认")]
    StopUnconfirmed,
}
#[derive(Clone)]
pub struct Limits {
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub stop_timeout: Duration,
    pub max_frame_bytes: usize,
    pub max_stderr_bytes: usize,
    pub max_tools: usize,
    pub max_concurrent_calls: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            startup_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            stop_timeout: Duration::from_secs(5),
            max_frame_bytes: 512 * 1024,
            max_stderr_bytes: 64 * 1024,
            max_tools: 128,
            max_concurrent_calls: 4,
        }
    }
}
impl Limits {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if [
            self.startup_timeout,
            self.request_timeout,
            self.stop_timeout,
        ]
        .iter()
        .any(|v| v.is_zero() || *v > Duration::from_secs(300))
            || !(1024..=1024 * 1024).contains(&self.max_frame_bytes)
            || self.max_stderr_bytes > 64 * 1024
            || !(1..=128).contains(&self.max_tools)
            || !(1..=8).contains(&self.max_concurrent_calls)
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReplyKind {
    Complete,
    InputRequired,
    Task,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    pub kind: ReplyKind,
    pub value: Value,
}
pub struct Client {
    peer: Peer<RoleClient>,
    service: tokio::sync::Mutex<Option<RunningService<RoleClient, ClientConfig>>>,
    control: Arc<SessionControl>,
    limits: Limits,
    permits: Arc<Semaphore>,
    info: Value,
}
fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 128 && !value.chars().any(char::is_control)
}
fn bounded_json(value: &Value, max: usize) -> bool {
    let mut stack = vec![(value, 0usize)];
    let mut nodes = 0usize;
    while let Some((value, depth)) = stack.pop() {
        nodes += 1;
        if depth > 32 || nodes > 4096 {
            return false;
        }
        match value {
            Value::Array(values) => stack.extend(values.iter().map(|v| (v, depth + 1))),
            Value::Object(values) => stack.extend(values.values().map(|v| (v, depth + 1))),
            _ => {}
        }
    }
    serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= max)
}
struct PendingRequest {
    handle: Option<RequestHandle<RoleClient>>,
    permit: Option<OwnedSemaphorePermit>,
    control: Arc<SessionControl>,
}
fn cleanup_request(
    handle: RequestHandle<RoleClient>,
    permit: Option<OwnedSemaphorePermit>,
    control: Arc<SessionControl>,
    reason: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _permit = permit;
        if !matches!(
            tokio::time::timeout(
                Duration::from_millis(200),
                handle.cancel(Some(reason.into()))
            )
            .await,
            Ok(Ok(()))
        ) {
            control.fault.store(true, Ordering::SeqCst);
            control.stop.cancel();
        }
    })
}
impl PendingRequest {
    async fn cancel(mut self, reason: &'static str) {
        if let Some(handle) = self.handle.take() {
            let cleanup = cleanup_request(handle, self.permit.take(), self.control.clone(), reason);
            let _ = cleanup.await;
        }
    }
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                cleanup_request(
                    handle,
                    self.permit.take(),
                    self.control.clone(),
                    "call future dropped",
                );
            } else {
                self.control.stop.cancel();
            }
        }
    }
}
impl Client {
    pub(crate) async fn from_service(
        service: RunningService<RoleClient, ClientConfig>,
        limits: Limits,
        control: Arc<SessionControl>,
    ) -> Result<Self, Error> {
        service
            .peer()
            .set_response_cache_config(ClientCacheConfig::disabled())
            .await;
        let peer = service.peer().clone();
        let info = serde_json::to_value(peer.peer_info()).map_err(|_| Error::StartRejected)?;
        Ok(Self {
            peer,
            service: tokio::sync::Mutex::new(Some(service)),
            control,
            permits: Arc::new(Semaphore::new(limits.max_concurrent_calls)),
            limits,
            info,
        })
    }
    pub fn is_closed(&self) -> bool {
        self.control.stop.is_cancelled() || self.peer.is_transport_closed()
    }

    pub fn info(&self) -> &Value {
        &self.info
    }
    pub async fn close(&self) {
        self.control.stop.cancel();
        if let Some(mut service) = self.service.lock().await.take() {
            let _ = service.close_with_timeout(self.limits.stop_timeout).await;
        }
    }
    async fn permit(&self, cancel: &CancellationToken) -> Result<OwnedSemaphorePermit, Error> {
        if cancel.is_cancelled() {
            return Err(Error::CancelledBeforeSend);
        }
        if self.is_closed() {
            return Err(Error::Unavailable);
        }
        self.permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::BusyBeforeSend)
    }
    async fn request(
        &self,
        request: ClientRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerResult, Error> {
        let permit = self.permit(cancel).await?;
        let handle = tokio::time::timeout(
            self.limits.request_timeout,
            self.peer
                .send_cancellable_request(request, PeerRequestOptions::no_options()),
        )
        .await
        .map_err(|_| Error::TimeoutUnknown)?
        .map_err(safe_error)?;
        let mut pending = PendingRequest {
            handle: Some(handle),
            permit: Some(permit),
            control: self.control.clone(),
        };
        let response = tokio::select! {
            response = &mut pending.handle.as_mut().unwrap().rx=>response,
            _ = cancel.cancelled()=>{pending.cancel("caller cancelled").await;return Err(Error::CancelledUnknown)},
            _ = self.control.stop.cancelled()=>{pending.cancel("process stopped").await;return Err(Error::TransportUnknown)},
            _ = tokio::time::sleep(self.limits.request_timeout)=>{pending.cancel("request deadline").await;return Err(Error::TimeoutUnknown)}
        };
        pending.handle.take();
        response
            .map_err(|_| Error::TransportUnknown)?
            .map_err(safe_error)
    }
    pub async fn tools(&self, cancel: CancellationToken) -> Result<Vec<Value>, Error> {
        let mut tools = Vec::new();
        let mut cursors = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut cursor = None;
        for _ in 0..16 {
            let result = self
                .request(
                    ClientRequest::ListToolsRequest(ListToolsRequest::with_param(
                        PaginatedRequestParams::default().with_cursor(cursor),
                    )),
                    &cancel,
                )
                .await?;
            let ServerResult::ListToolsResult(result) = result else {
                return Err(Error::TransportUnknown);
            };
            for tool in result.tools {
                if !valid_name(&tool.name)
                    || !names.insert(tool.name.to_string())
                    || tool.input_schema.get("type").and_then(Value::as_str) != Some("object")
                {
                    return Err(Error::Limit);
                }
                tools.push(serde_json::to_value(tool).map_err(|_| Error::Limit)?);
                if tools.len() > self.limits.max_tools {
                    return Err(Error::Limit);
                }
            }
            if !bounded_json(&Value::Array(tools.clone()), 256 * 1024) {
                return Err(Error::Limit);
            }
            match result.next_cursor {
                None => return Ok(tools),
                Some(next) if next.len() <= 1024 && cursors.insert(next.clone()) => {
                    cursor = Some(next)
                }
                _ => return Err(Error::Limit),
            }
        }
        Err(Error::Limit)
    }
    pub async fn call(
        &self,
        name: &str,
        arguments: Value,
        cancel: CancellationToken,
    ) -> Result<Reply, Error> {
        if cancel.is_cancelled() {
            return Err(Error::CancelledBeforeSend);
        }
        if !valid_name(name) || !arguments.is_object() || !bounded_json(&arguments, 64 * 1024) {
            return Err(Error::InvalidInput);
        }
        let params = CallToolRequestParams::new(name.to_owned())
            .with_arguments(arguments.as_object().unwrap().clone());
        let result = self
            .request(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                &cancel,
            )
            .await?;
        let (kind, value) = match result {
            ServerResult::CallToolResult(result) => {
                (ReplyKind::Complete, serde_json::to_value(result))
            }
            ServerResult::InputRequiredResult(result) => {
                (ReplyKind::InputRequired, serde_json::to_value(result))
            }
            ServerResult::CreateTaskResult(result) => {
                (ReplyKind::Task, serde_json::to_value(result))
            }
            _ => return Err(Error::TransportUnknown),
        };
        let value = value.map_err(|_| Error::TransportUnknown)?;
        if !bounded_json(&value, self.limits.max_frame_bytes) {
            return Err(Error::TransportUnknown);
        }
        Ok(Reply { kind, value })
    }
}
fn safe_error(error: ServiceError) -> Error {
    match error {
        ServiceError::McpError(error) if [-32600, -32601, -32602].contains(&error.code.0) => {
            Error::RemoteRejected { code: error.code.0 }
        }
        ServiceError::McpError(error) => Error::RemoteUnknown { code: error.code.0 },
        _ => Error::TransportUnknown,
    }
}
