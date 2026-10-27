//! Trusted host programs only; HTTP request input is not command authority.
use crate::{
    session::{Client, Error, Limits, SessionControl},
    transport::BoundedTransport,
};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use rmcp::{
    model::{ClientCapabilities, ClientConfig, Implementation, ProtocolVersion},
    service::{ClientLifecycleMode, ClientServiceExt},
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncReadExt, sync::watch};
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
    rpc: Arc<SessionControl>,
    requested: AtomicBool,
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
        self.control.rpc.stop.cancel();
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
            rpc: SessionControl::new(),
            requested: AtomicBool::new(false),
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
            control.rpc.stop.clone(),
            control.rpc.fault.clone(),
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
        state.transport_fault = self.control.rpc.fault.load(Ordering::SeqCst);
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
        self.control.rpc.stop.cancel();
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
                if self.control.rpc.stop.is_cancelled() {
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
                Client::from_service(service, self.limits.clone(), self.control.rpc.clone()).await
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
            _ = control.rpc.stop.cancelled()=>break,
            _ = ticker.tick()=>{if parent_exited(child.as_mut(),pid){break}}
        }
    }
    let mut stopping = state.borrow().clone();
    stopping.phase = Phase::Stopping;
    stopping.stop_requested = control.requested.load(Ordering::SeqCst);
    stopping.transport_fault = control.rpc.fault.load(Ordering::SeqCst);
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
