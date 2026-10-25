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

#[tokio::test]
async fn unsupported_methods_keep_allow_headers_request_ids_and_the_standard_error_envelope() {
    use admin_api::{build_router, config::Config, state::AppState};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let config = Config::from_lookup(|name| {
        (name == "TRI_JWT_SIGNING_KEY")
            .then(|| "response-fixture-key-at-least-thirty-two-bytes".into())
    })
    .unwrap();
    let router = build_router(AppState::new(config, None));
    for (method, path, allowed) in [
        ("POST", "/health/live", "GET"),
        ("PUT", "/api/v1/users", "POST"),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 405);
        assert!(
            response
                .headers()
                .get("allow")
                .unwrap()
                .to_str()
                .unwrap()
                .split(',')
                .map(str::trim)
                .any(|value| value == allowed)
        );
        assert!(response.headers().contains_key("x-request-id"));
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["error"]["code"], "METHOD_NOT_ALLOWED");
    }
    let response = router
        .oneshot(
            Request::builder()
                .uri("/missing-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn malformed_json_is_bad_request_and_never_echoes_supplied_secret_text() {
    use admin_api::{build_router, config::Config, state::AppState};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let config = Config::from_lookup(|name| {
        (name == "TRI_JWT_SIGNING_KEY")
            .then(|| "response-fixture-key-at-least-thirty-two-bytes".into())
    })
    .unwrap();
    let router = build_router(AppState::new(config, None));
    for path in [
        "/api/v1/auth/login",
        "/api/v1/auth/refresh",
        "/api/v1/auth/logout",
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from("{\"refreshToken\":\"DO-NOT-ECHO-SECRET"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{path}");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["error"]["code"], "BAD_REQUEST");
        assert!(!body.to_string().contains("DO-NOT-ECHO-SECRET"));
    }
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/refresh")
                .header("content-type", "application/json")
                .body(Body::from("{\"refreshToken\":null}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn json_content_type_errors_keep_their_standard_http_status() {
    use admin_api::{build_router, config::Config, state::AppState};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let config = Config::from_lookup(|name| {
        (name == "TRI_JWT_SIGNING_KEY")
            .then(|| "response-fixture-key-at-least-thirty-two-bytes".into())
    })
    .unwrap();
    let router = build_router(AppState::new(config, None));
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 415);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "UNSUPPORTED_MEDIA_TYPE");
    for content_type in [
        "application/json; charset=utf-8",
        "application/vnd.tri-admin+json",
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/refresh")
                    .header("content-type", content_type)
                    .body(Body::from("{\"refreshToken\":null}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 422, "{content_type}");
    }
}

#[tokio::test]
async fn oversized_json_uses_payload_too_large_instead_of_field_validation() {
    use admin_api::{build_router, config::Config, state::AppState};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let config = Config::from_lookup(|name| {
        (name == "TRI_JWT_SIGNING_KEY")
            .then(|| "response-fixture-key-at-least-thirty-two-bytes".into())
    })
    .unwrap();
    let router = build_router(AppState::new(config, None));
    let payload = format!("{{\"refreshToken\":\"{}\"}}", "x".repeat(2 * 1024 * 1024));
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header("content-type", "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "PAYLOAD_TOO_LARGE");
}
