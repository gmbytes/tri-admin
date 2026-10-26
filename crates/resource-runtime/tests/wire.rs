use mongodb::Client;
use serde_json::json;
use tri_resource_runtime::{Config, State, router, schema::Schema};
#[tokio::test]
async fn actual_http_auth_and_resource_contract_revoke_and_protect_origins() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let env = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.env")).unwrap();
    let uri = env
        .lines()
        .find_map(|line| line.strip_prefix("TRI_MONGODB_URI="))
        .unwrap();
    let database = Client::with_uri_str(uri).await.unwrap().database(&format!(
        "tri_resource_test_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let schema=Schema::parse(json!({"resource":"products","label":"产品","fields":[{"name":"name","label":"名称","kind":"text","required":true,"searchable":true}]})).unwrap();
    let state = State::new(
        database.clone(),
        schema,
        Config::new(
            "wire-fixture-resource-key-at-least-32-bytes".into(),
            "http://localhost:5173".into(),
        )
        .unwrap(),
    )
    .unwrap();
    state.migrate().await.unwrap();
    let password = format!("fixture-{}", uuid::Uuid::new_v4());
    state
        .provision_admin("owner", &password, true, true)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let ready = client
        .get(format!("{origin}/health/ready"))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);
    assert_eq!(ready.headers()["cache-control"], "private, no-store");
    assert!(ready.headers().contains_key("x-request-id"));
    let response = client
        .post(format!("{origin}/api/v1/auth/login"))
        .header("origin", "http://localhost:5173")
        .header("content-type", "application/json")
        .body(json!({"username":"owner","password":password}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "http://localhost:5173"
    );
    let signed: serde_json::Value =
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let access = signed["data"]["accessToken"].as_str().unwrap();
    let create = client
        .post(format!("{origin}/api/v1/products"))
        .bearer_auth(access)
        .header("content-type", "application/json")
        .body(json!({"values":{"name":"HTTP产品"}}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 201);
    let row: serde_json::Value = serde_json::from_slice(&create.bytes().await.unwrap()).unwrap();
    assert_eq!(row["data"]["values"]["name"], "HTTP产品");
    let blocked = client
        .get(format!("{origin}/api/v1/products"))
        .header("origin", "https://untrusted.example")
        .bearer_auth(access)
        .send()
        .await
        .unwrap();
    assert!(
        blocked
            .headers()
            .get("access-control-allow-origin")
            .is_none_or(|value| value != "https://untrusted.example")
    );
    let logout = client
        .post(format!("{origin}/api/v1/auth/logout"))
        .bearer_auth(access)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);
    assert_eq!(
        client
            .get(format!("{origin}/api/v1/products"))
            .bearer_auth(access)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    tx.send(()).unwrap();
    server.await.unwrap();
    database.drop().await.unwrap();
}
