use admin_api::{
    bootstrap::{AdminInput, bootstrap_database},
    build_router,
    config::Config,
    state::AppState,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::{Client, Database};
use serde_json::Value;
use tower::ServiceExt;

pub const PASSWORD: &str = "FixtureAdminPassword-2026";
pub const KEY: &str = "fixture-signing-key-at-least-32-bytes-long";

pub struct TestApp {
    pub router: Router,
    pub database: Database,
}

impl TestApp {
    pub async fn new() -> Self {
        Self::with_ip_limit(100).await
    }

    pub async fn with_ip_limit(limit: i64) -> Self {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env");
        let env: std::collections::HashMap<_, _> = dotenvy::from_path_iter(path)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let client = Client::with_uri_str(env.get("TRI_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let database =
            client.database(&format!("tri_admin_test_{}", uuid::Uuid::new_v4().simple()));
        bootstrap_database(
            &database,
            &AdminInput {
                username: "admin".into(),
                password: PASSWORD.into(),
            },
        )
        .await
        .unwrap();
        let config = Config::from_lookup(|name| match name {
            "TRI_JWT_SIGNING_KEY" => Some(KEY.into()),
            "TRI_LOGIN_LIMIT_PER_IP" => Some(limit.to_string()),
            "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
            "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
            _ => None,
        })
        .unwrap();
        Self {
            router: build_router(AppState::new(config, Some(database.clone()))),
            database,
        }
    }

    pub async fn send(
        &self,
        method: &str,
        path: &str,
        body: Value,
        access: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(token) = access {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .router
            .clone()
            .oneshot(
                request
                    .body(if body.is_null() {
                        Body::empty()
                    } else {
                        Body::from(body.to_string())
                    })
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, json)
    }

    pub async fn login(&self) -> Value {
        let (status, body) = self
            .send(
                "POST",
                "/api/v1/auth/login",
                serde_json::json!({"username":"admin","password":PASSWORD}),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "login failed: {body}");
        body["data"].clone()
    }

    pub async fn cleanup(self) {
        self.database.drop().await.unwrap();
    }
}
