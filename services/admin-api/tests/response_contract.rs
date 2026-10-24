use admin_api::common::{error::AppError, response::ApiResponse};
use axum::{body::to_bytes, response::IntoResponse};
use serde_json::{Value, json};

#[tokio::test]
async fn success_returns_new_data_envelope_and_camel_case_fields() {
    let response = ApiResponse::success(json!({"id":"abc","createdAt":"fixture"})).into_response();
    assert_eq!(response.status(), 200);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body, json!({"data":{"id":"abc","createdAt":"fixture"}}));
}

#[tokio::test]
async fn authentication_and_permission_errors_use_standard_http_statuses() {
    let unauthorized = AppError::Unauthorized.into_response();
    assert_eq!(unauthorized.status(), 401);
    let denied = AppError::Forbidden.into_response();
    assert_eq!(denied.status(), 403);
    let body: Value =
        serde_json::from_slice(&to_bytes(denied.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"error":{"code":"FORBIDDEN","message":"权限不足"}})
    );
}
