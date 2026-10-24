use admin_api::{build_router, config::Config, state::AppState};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use serde_json::Value;
use tower::ServiceExt;

fn config() -> Config {
    Config::from_lookup(|name| match name {
        "TRI_JWT_SIGNING_KEY" => Some("test-only-signing-key-with-at-least-32-characters".into()),
        _ => None,
    })
    .unwrap()
}

#[tokio::test]
async fn health_is_available_without_database_but_readiness_is_not() {
    let app = build_router(AppState::new(config(), None));
    let health = app
        .clone()
        .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    assert!(health.headers().contains_key("x-request-id"));
    let live: Value =
        serde_json::from_slice(&to_bytes(health.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(live["data"]["status"], "ok");
    let ready = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), 503);
    let body: Value =
        serde_json::from_slice(&to_bytes(ready.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "SERVICE_NOT_READY");
}

#[test]
fn weak_signing_keys_are_rejected_without_echoing_them() {
    let error =
        Config::from_lookup(|name| (name == "TRI_JWT_SIGNING_KEY").then(|| "short-secret".into()))
            .unwrap_err();
    assert!(!error.to_string().contains("short-secret"));
}

#[test]
fn configuration_debug_output_redacts_credentials() {
    let config = Config::from_lookup(|name| match name {
        "TRI_JWT_SIGNING_KEY" => Some("not-a-real-signing-key-but-long-enough".into()),
        "TRI_MONGODB_URI" => {
            Some("mongodb://root:private-password@127.0.0.1:13317/?authSource=admin".into())
        }
        _ => None,
    })
    .unwrap();
    let debug = format!("{config:?}");
    assert!(!debug.contains("private-password"));
    assert!(!debug.contains("not-a-real-signing-key-but-long-enough"));
}
