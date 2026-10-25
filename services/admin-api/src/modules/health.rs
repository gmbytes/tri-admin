use crate::{
    common::{error::AppError, response::ApiResponse},
    state::AppState,
};
use axum::{Json, extract::State};
use mongodb::bson::doc;
use serde_json::{Value, json};

crate::api_routes! {
    prefix "";
    get "/health/live" => live, "health.live", "health", "进程存活", Public, &[];
    get "/health/ready" => ready, "health.ready", "health", "服务就绪", Public, &[];
}

pub async fn live() -> ApiResponse<Value> {
    ApiResponse::success(json!({"status":"ok"}))
}

pub async fn ready(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let database = state.require_database()?;
    let initialized = async {
        database.run_command(doc! {"ping":1}).await?;
        crate::storage::schema::is_initialized(database).await
    };
    match tokio::time::timeout(std::time::Duration::from_secs(3), initialized).await {
        Ok(Ok(true)) => Ok(Json(json!({"data":{"status":"ready"}}))),
        _ => Err(AppError::NotReady),
    }
}
