use crate::{
    common::{error::AppError, response::ApiResponse},
    state::AppState,
};
use axum::{Json, extract::State};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};

pub async fn live() -> ApiResponse<Value> {
    ApiResponse::success(json!({"status":"ok"}))
}

pub async fn ready(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let database = state.require_database()?;
    let initialized = async {
        database.run_command(doc! {"ping":1}).await?;
        database
            .collection::<Document>("_schema_migrations")
            .find_one(doc! {"_id":"core","version":1,"bootstrapComplete":true})
            .await
    };
    match tokio::time::timeout(std::time::Duration::from_secs(3), initialized).await {
        Ok(Ok(Some(_))) => Ok(Json(json!({"data":{"status":"ready"}}))),
        _ => Err(AppError::NotReady),
    }
}
