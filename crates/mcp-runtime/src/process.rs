//! Trusted host programs only. This core does not grant an HTTP caller command authority.
//! The containing service must resolve deployment capabilities and current user permissions.
use crate::transport::BoundedTransport;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use rmcp::{
    RoleClient,
    model::{
        CallToolRequest, CallToolRequestParams, ClientCapabilities, ClientConfig, ClientRequest,
        Implementation, ListToolsRequest, PaginatedRequestParams, ProtocolVersion, ServerResult,
    },
    service::{
        ClientCacheConfig, ClientLifecycleMode, ClientServiceExt, Peer, PeerRequestOptions,
        RequestHandle, RunningService, ServiceError,
    },
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    sync::{OwnedSemaphorePermit, Semaphore, watch},
};
use tokio_util::sync::CancellationToken;
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("MCP 程序或运行限制无效")]
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
    fn validate(&self) -> Result<(), Error> {
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
// Intentionally no Debug/Serialize: environment may contain explicit integration credentials.
pub struct ProgramSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
}
impl ProgramSpec {
    fn validate(&self) -> Result<(), Error> {
        if !self.program.is_absolute()
            || !self.program.is_file()
            || !self.cwd.is_absolute()
            || !self.cwd.is_dir()
            || self.args.len() > 64
            || self.args.iter().any(|v| v.contains('\0'))
            || self.args.iter().map(String::len).sum::<usize>() > 16 * 1024
            || self.environment.len() > 32
            || self.environment.iter().any(|(k, v)| {
                k.is_empty()
                    || k.len() > 128
                    || !k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    || v.contains('\0')
            })
            || self
                .environment
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
                > 16 * 1024
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Starting,
    Running,
    Stopping,
    Exited,
    Unconfirmed,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessState {
    pub phase: Phase,
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub stop_requested: bool,
    pub transport_fault: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StderrSnapshot {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}
struct Control {
    stop: CancellationToken,
    requested: AtomicBool,
    fault: Arc<AtomicBool>,
    stderr: Arc<Mutex<StderrSnapshot>>,
}
pub struct Process {
    control: Arc<Control>,
    state: watch::Receiver<ProcessState>,
    state_sender: watch::Sender<ProcessState>,
    transport: Option<BoundedTransport>,
    limits: Limits,
}
impl Drop for Process {
    fn drop(&mut self) {
        self.control.requested.store(true, Ordering::SeqCst);
        self.control.stop.cancel();
    }
}
impl Process {
    pub fn spawn(spec: ProgramSpec, limits: Limits) -> Result<Self, Error> {
        spec.validate()?;
        limits.validate()?;
        let mut command = CommandWrap::with_new(&spec.program, |cmd| {
            cmd.args(&spec.args)
                .current_dir(&spec.cwd)
                .env_clear()
                .envs(&spec.environment)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        });
        command.wrap(KillOnDrop);
        #[cfg(windows)]
        {
            use process_wrap::tokio::{CreationFlags, JobObject};
            use windows::Win32::System::Threading::CREATE_NO_WINDOW;
            command.wrap(CreationFlags(CREATE_NO_WINDOW));
            command.wrap(JobObject);
        }
        #[cfg(unix)]
        command.wrap(process_wrap::tokio::ProcessGroup::leader());
        let mut child = command.spawn().map_err(|_| Error::Spawn)?;
        let pid = child.id().ok_or(Error::Spawn)?;
        let stdout = child.stdout().take().ok_or(Error::Spawn)?;
        let stdin = child.stdin().take().ok_or(Error::Spawn)?;
        let stderr = child.stderr().take().ok_or(Error::Spawn)?;
        let control = Arc::new(Control {
            stop: CancellationToken::new(),
            requested: AtomicBool::new(false),
            fault: Arc::new(AtomicBool::new(false)),
            stderr: Arc::new(Mutex::new(StderrSnapshot {
                bytes: Vec::new(),
                truncated: false,
            })),
        });
        let (state_tx, state) = watch::channel(ProcessState {
            phase: Phase::Starting,
            pid,
            exit_code: None,
            stop_requested: false,
            transport_fault: false,
        });
        let transport = BoundedTransport::new(
            stdout,
            stdin,
            limits.max_frame_bytes,
            control.stop.clone(),
            control.fault.clone(),
        );
        let capture = control.stderr.clone();
        let max = limits.max_stderr_bytes;
        let reader = tokio::spawn(async move {
            let mut stderr = stderr;
            let mut block = [0; 4096];
            while let Ok(n) = stderr.read(&mut block).await {
                if n == 0 {
                    break;
                }
                let mut record = capture.lock().unwrap_or_else(|v| v.into_inner());
                let remaining = max.saturating_sub(record.bytes.len());
                record.bytes.extend_from_slice(&block[..n.min(remaining)]);
                record.truncated |= n > remaining;
            }
        });
        let monitor = control.clone();
        let state_sender = state_tx.clone();
        tokio::spawn(async move {
            supervise(child, monitor, state_tx).await;
            let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
        });
        Ok(Self {
            control,
            state,
            state_sender,
            transport: Some(transport),
            limits,
        })
    }
    pub fn state(&self) -> ProcessState {
        let mut state = self.state.borrow().clone();
        state.stop_requested = self.control.requested.load(Ordering::SeqCst);
        state.transport_fault = self.control.fault.load(Ordering::SeqCst);
        state
    }
    pub fn subscribe(&self) -> watch::Receiver<ProcessState> {
        self.state.clone()
    }
    pub fn stderr(&self) -> StderrSnapshot {
        self.control
            .stderr
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .clone()
    }
    pub async fn stop(&self) -> Result<ProcessState, Error> {
        self.control.requested.store(true, Ordering::SeqCst);
        self.control.stop.cancel();
        let mut state = self.state.clone();
        tokio::time::timeout(self.limits.stop_timeout, async {
            loop {
                let current = state.borrow().clone();
                if current.phase == Phase::Exited {
                    return Ok(self.state());
                }
                if current.phase == Phase::Unconfirmed {
                    return Err(Error::StopUnconfirmed);
                }
                if state.changed().await.is_err() {
                    return Err(Error::StopUnconfirmed);
                }
            }
        })
        .await
        .map_err(|_| Error::StopUnconfirmed)?
    }
    pub async fn connect(&mut self) -> Result<Client, Error> {
        let transport = self.transport.take().ok_or(Error::Unavailable)?;
        let config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("tri-admin", env!("CARGO_PKG_VERSION")),
        );
        let connected = tokio::time::timeout(
            self.limits.startup_timeout,
            config.serve_with_lifecycle(
                transport,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            ),
        )
        .await;
        match connected {
            Ok(Ok(service)) => {
                if self.control.stop.is_cancelled() {
                    let _ = self.stop().await;
                    return Err(Error::Unavailable);
                }
                if !self.state_sender.send_if_modified(|state| {
                    if state.phase == Phase::Starting {
                        state.phase = Phase::Running;
                        true
                    } else {
                        false
                    }
                }) {
                    self.stop().await?;
                    return Err(Error::Unavailable);
                }
                service
                    .peer()
                    .set_response_cache_config(ClientCacheConfig::disabled())
                    .await;
                let peer = service.peer().clone();
                let info =
                    serde_json::to_value(peer.peer_info()).map_err(|_| Error::StartRejected)?;
                Ok(Client {
                    peer,
                    service: tokio::sync::Mutex::new(Some(service)),
                    control: self.control.clone(),
                    limits: self.limits.clone(),
                    permits: Arc::new(Semaphore::new(self.limits.max_concurrent_calls)),
                    info,
                })
            }
            failure => {
                let error = if failure.is_err() {
                    Error::StartTimeout
                } else {
                    Error::StartRejected
                };
                self.stop().await?;
                Err(error)
            }
        }
    }
}
async fn supervise(
    mut child: Box<dyn ChildWrapper>,
    control: Arc<Control>,
    state: watch::Sender<ProcessState>,
) {
    let mut ticker = tokio::time::interval(Duration::from_millis(20));
    let pid = state.borrow().pid;
    loop {
        tokio::select! {
            _ = control.stop.cancelled()=>break,
            _ = ticker.tick()=>{if parent_exited(child.as_mut(),pid){break}}
        }
    }
    let mut stopping = state.borrow().clone();
    stopping.phase = Phase::Stopping;
    stopping.stop_requested = control.requested.load(Ordering::SeqCst);
    stopping.transport_fault = control.fault.load(Ordering::SeqCst);
    state.send_replace(stopping.clone());
    let _ = child.start_kill();
    match child.wait().await {
        Ok(status) => {
            #[cfg(unix)]
            if confirm_group_exit(pid).await.is_err() {
                stopping.phase = Phase::Unconfirmed;
                state.send_replace(stopping);
                return;
            }
            stopping.phase = Phase::Exited;
            stopping.exit_code = status.code();
        }
        Err(_) => stopping.phase = Phase::Unconfirmed,
    }
    state.send_replace(stopping);
}
fn parent_exited(child: &mut dyn ChildWrapper, pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::{
            sys::wait::{Id, WaitPidFlag, WaitStatus, waitid},
            unistd::Pid,
        };
        let _ = child;
        // Observe without reaping: retain the leader PID until the owned group is killed.
        matches!(
            waitid(
                Id::Pid(Pid::from_raw(pid as i32)),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT
            ),
            Ok(WaitStatus::Exited(..) | WaitStatus::Signaled(..))
        )
    }
    #[cfg(windows)]
    {
        let _ = pid;
        // Only one OS child wrapper is installed. Its try_wait would drain job events;
        // observe the native parent here and retain the wrapper's wait for tree exit.
        child.inner_mut().try_wait().ok().flatten().is_some()
    }
}
#[cfg(unix)]
async fn confirm_group_exit(pid: u32) -> Result<(), ()> {
    use nix::{errno::Errno, sys::signal::killpg, unistd::Pid};
    loop {
        match killpg(Pid::from_raw(pid as i32), None) {
            Err(Errno::ESRCH) => return Ok(()),
            Ok(()) => tokio::time::sleep(Duration::from_millis(10)).await,
            Err(_) => return Err(()),
        }
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
    control: Arc<Control>,
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
    control: Arc<Control>,
}
fn cleanup_request(
    handle: RequestHandle<RoleClient>,
    permit: Option<OwnedSemaphorePermit>,
    control: Arc<Control>,
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
        if self.control.stop.is_cancelled() {
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
        ServiceError::McpError(error) => Error::RemoteRejected { code: error.code.0 },
        _ => Error::TransportUnknown,
    }
}
