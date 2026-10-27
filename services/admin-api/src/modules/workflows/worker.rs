use crate::{common::error::AppError, state::AppState};
use std::time::Duration;
pub async fn run_once(state: &AppState) -> Result<bool, AppError> {
    let Ok(_slot) = state.workflow_worker_slots.clone().try_acquire_owned() else {
        return Ok(false);
    };
    if !crate::storage::schema::is_initialized(state.require_database()?).await? {
        return Err(AppError::NotReady);
    }
    super::runs::expire(state).await?;
    let Some((row, token)) = super::runs::claim(state).await? else {
        return Ok(false);
    };
    if let Err(error) = super::runs::tick(state, &row, &token).await {
        super::runs::interrupted(state, &row, &token, &error).await?;
    }
    Ok(true)
}
pub fn start_service(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let state = state.clone();
            tasks.spawn(async move{while !state.workflow_worker_shutdown.is_cancelled(){if let Err(error)=run_once(&state).await&&!matches!(error,AppError::NotReady){tracing::warn!(code="WORKFLOW_DEFERRED","workflow iteration deferred");}tokio::select!{_=state.workflow_worker_shutdown.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(300))=>{}}}});
        }
        while tasks.join_next().await.is_some() {}
    })
}
pub fn stop_service(state: &AppState) {
    state.workflow_worker_shutdown.cancel();
}
