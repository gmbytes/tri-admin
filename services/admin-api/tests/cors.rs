use admin_api::{build_router, config::Config, state::AppState};
use axum::{body::Body, http::Request};
use tower::ServiceExt;

async fn allowed(origin: &str, configured: Option<&str>) -> bool {
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some("fixture-signing-key-at-least-32-bytes".into()),
        "TRI_CORS_ORIGINS" => configured.map(str::to_owned),
        _ => None,
    })
    .unwrap();
    let response = build_router(AppState::new(config, None))
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/auth/login")
                .header("origin", origin)
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    response
        .headers()
        .get("access-control-allow-origin")
        .is_some_and(|value| value == origin)
}

#[tokio::test]
async fn browser_preview_defaults_allow_both_explicit_loopback_origins() {
    assert!(allowed("http://localhost:1420", None).await);
    assert!(allowed("http://127.0.0.1:1420", None).await);
    assert!(!allowed("https://untrusted.example", None).await);
}

#[tokio::test]
async fn explicit_cors_configuration_does_not_silently_add_development_origins() {
    assert!(allowed("https://admin.example", Some("https://admin.example")).await);
    assert!(!allowed("http://127.0.0.1:1420", Some("https://admin.example")).await);
}
