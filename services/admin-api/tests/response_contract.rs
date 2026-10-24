use admin_api::common::{error::AppError, response::ApiResponse};
use axum::{body::to_bytes, response::IntoResponse};
use serde_json::{Value, json};

#[tokio::test]
async fn success_preserves_existing_response_envelope_and_field_case() {
    let response = ApiResponse::success(json!({"ID":7,"CreatedAt":"fixture"})).into_response();
    assert_eq!(response.status(),200);
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(),4096).await.unwrap()).unwrap();
    assert_eq!(body,json!({"code":0,"data":{"ID":7,"CreatedAt":"fixture"},"msg":"成功"}));
}

#[tokio::test]
async fn authentication_failure_uses_401_while_business_failure_keeps_code_7() {
    let unauthorized = AppError::Unauthorized.into_response();
    assert_eq!(unauthorized.status(),401);
    let denied = AppError::Business("权限不足".into()).into_response();
    assert_eq!(denied.status(),200);
    let body: Value = serde_json::from_slice(&to_bytes(denied.into_body(),4096).await.unwrap()).unwrap();
    assert_eq!(body,json!({"code":7,"data":{},"msg":"权限不足"}));
}
