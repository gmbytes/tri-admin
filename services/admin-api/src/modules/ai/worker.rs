use super::journal::{self, Claim, Operation};
use crate::{common::error::AppError, modules::settings::network, state::AppState};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tri_ai_runtime::http::{Adapter, CreateRequest, FailureKind, HttpSpec, Policy, Reference};
fn code(error: &AppError) -> &'static str {
    match error {
        AppError::Forbidden | AppError::Unauthorized => "PERMISSION_REVOKED",
        AppError::SecretStoreUnavailable => "SECRET_STORE_UNAVAILABLE",
        AppError::Conflict(value) if value == "BINDING_CHANGED" => "BINDING_CHANGED",
        AppError::Conflict(value) if value == "LEASE_LOST" => "LEASE_LOST",
        AppError::Conflict(value) if value == "RUN_EXPIRED" => "RUN_EXPIRED",
        AppError::Conflict(value) if value == "CANCEL_REQUESTED" => "CANCEL_REQUESTED",
        AppError::Conflict(value) if value == "UNKNOWN_SUBMISSION" => "UNKNOWN_SUBMISSION",
        _ => "AI_EXECUTION_FAILED",
    }
}
struct ExecutionFailure {
    code: &'static str,
    definitely_unsent: bool,
}
impl From<AppError> for ExecutionFailure {
    fn from(error: AppError) -> Self {
        Self {
            code: code(&error),
            definitely_unsent: false,
        }
    }
}
impl From<tri_ai_runtime::http::Failure> for ExecutionFailure {
    fn from(error: tri_ai_runtime::http::Failure) -> Self {
        Self {
            code: match error.kind {
                FailureKind::Cancelled => "CANCELLED",
                FailureKind::Timeout => "UPSTREAM_TIMEOUT",
                FailureKind::Status(_) => "UPSTREAM_REJECTED",
                FailureKind::Input => "INVALID_BINDING",
                _ => "UPSTREAM_INTERRUPTED",
            },
            definitely_unsent: !error.may_have_been_sent,
        }
    }
}
async fn execute(
    state: &AppState,
    claim: &Claim,
    cancel: CancellationToken,
) -> Result<(), ExecutionFailure> {
    let prepared = journal::prepare(state, claim).await?;
    let expected = prepared.input.format;
    let streamed = prepared.row.stream
        && matches!(prepared.operation, Operation::Generate | Operation::Resume);
    let url = reqwest::Url::parse(&prepared.endpoint)
        .map_err(|_| AppError::Validation("AI 地址无效".into()))?;
    let client = network::http_client_with_timeout(&url, &state.config, Duration::from_secs(120))
        .await
        .map_err(|_| AppError::NotReady)?;
    let adapter = Adapter::new(
        HttpSpec {
            endpoint: prepared.endpoint,
            client,
            headers: prepared.headers,
        },
        tri_ai_runtime::Limits::default(),
        Policy::default(),
    )?;
    journal::mark(state, claim, prepared.operation).await?;
    let mut exchange = match prepared.operation {
        Operation::Generate => {
            adapter
                .create(
                    CreateRequest {
                        format: expected.map(|format| format.definition()),
                        model: prepared.input.model,
                        input: prepared.input.input,
                        instructions: prepared.input.instructions,
                        max_output_tokens: prepared.input.max_output_tokens,
                        background: true,
                        stream: prepared.input.stream,
                    },
                    cancel,
                )
                .await?
        }
        operation => {
            let id = prepared
                .response
                .response_id()
                .ok_or_else(|| AppError::Conflict("UNKNOWN_SUBMISSION".into()))?;
            let reference = Reference::new(id, true, prepared.row.stream)?;
            match operation {
                Operation::Cancel => {
                    adapter
                        .cancel(&reference, prepared.response, cancel)
                        .await?
                }
                Operation::Resume => {
                    adapter
                        .resume(&reference, prepared.response, cancel)
                        .await?
                }
                _ => {
                    adapter
                        .retrieve(&reference, prepared.response, cancel)
                        .await?
                }
            }
        }
    };
    let mut dirty = false;
    let mut pending_bytes = 0usize;
    let mut flush = tokio::time::interval(Duration::from_millis(200));
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    flush.tick().await;
    loop {
        // Cancelling next() is safe for SSE: parser state lives in the stream.
        // JSON consumes its body in one operation and never uses this timer.
        let next = if streamed && dirty {
            tokio::select! {_=flush.tick()=>None,next=exchange.next()=>Some(next)}
        } else {
            Some(exchange.next().await)
        };
        let Some(next) = next else {
            journal::persist(state, claim, exchange.state(), expected).await?;
            dirty = false;
            pending_bytes = 0;
            flush.reset();
            continue;
        };
        match next {
            Ok(Some(observation)) => {
                let immediate = match observation {
                    tri_ai_runtime::http::Observation::Event(tri_ai_runtime::Update::Ignored) => {
                        continue;
                    }
                    tri_ai_runtime::http::Observation::Event(tri_ai_runtime::Update::Delta {
                        text,
                        ..
                    }) => {
                        pending_bytes = pending_bytes.saturating_add(text.len());
                        false
                    }
                    tri_ai_runtime::http::Observation::Event(
                        tri_ai_runtime::Update::Progress { .. },
                    ) => false,
                    _ => true,
                };
                dirty = true;
                if immediate || pending_bytes >= 32 * 1024 {
                    journal::persist(state, claim, exchange.state(), expected).await?;
                    dirty = false;
                    pending_bytes = 0;
                    flush.reset();
                }
            }
            Ok(None) => {
                if dirty {
                    journal::persist(state, claim, exchange.state(), expected).await?;
                }
                break;
            }
            Err(error) => {
                if dirty {
                    journal::persist(state, claim, exchange.state(), expected).await?;
                }
                return Err(error.into());
            }
        }
    }
    journal::release(state, claim, None, false).await?;
    Ok(())
}
pub async fn run_once(state: &AppState) -> Result<bool, AppError> {
    let Ok(_slot) = state.ai_worker_slots.clone().try_acquire_owned() else {
        return Ok(false);
    };
    if !crate::storage::schema::is_initialized(state.require_database()?).await? {
        return Err(AppError::NotReady);
    }
    journal::expire(state).await?;
    let Some(claim) = journal::claim(state).await? else {
        return Ok(false);
    };
    let cancel = state.ai_worker_shutdown.child_token();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
    heartbeat.tick().await;
    let work = execute(state, &claim, cancel.clone());
    tokio::pin!(work);
    let mut control = None;
    let result = loop {
        tokio::select! {result=&mut work=>break result,_=state.ai_worker_shutdown.cancelled()=>{control=Some("SERVICE_STOPPED");cancel.cancel();break work.await;},_=heartbeat.tick()=>{if let Err(error)=journal::renew(state,&claim).await{if control.is_none(){control=Some(code(&error));}cancel.cancel();}}}
    };
    if let Err(error) = result
        && error.code != "LEASE_LOST"
    {
        journal::release(
            state,
            &claim,
            Some(control.unwrap_or(error.code)),
            error.definitely_unsent,
        )
        .await?;
    }
    Ok(true)
}
pub fn start_service(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let state = state.clone();
            tasks.spawn(async move{while !state.ai_worker_shutdown.is_cancelled(){if let Err(error)=run_once(&state).await&& !matches!(error,AppError::NotReady){tracing::warn!(code="AI_WORKER_DEFERRED","AI worker iteration deferred");}tokio::select!{_=state.ai_worker_shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(500))=>{}}}});
        }
        while tasks.join_next().await.is_some() {}
    })
}
pub fn stop_service(state: &AppState) {
    state.ai_worker_shutdown.cancel();
}
