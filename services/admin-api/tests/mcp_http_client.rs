mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::json;
use support::{PASSWORD, TestApp};
#[tokio::test]
async fn actual_http_client_uses_the_admin_mcp_identity_and_live_permissions() {
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;
    use tri_mcp_runtime::{
        http::{HttpSpec, connect},
        session::Limits,
    };
    let app = TestApp::new().await;
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap();
    let (status,role)=app.send("POST","/api/v1/roles",json!({"name":"HTTP RPC reader","slug":"http-rpc-reader","permissions":["mcp.read","mcp.invoke","parameters.read"],"dataScope":"self"}),Some(admin)).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let role_id = role["data"]["id"].as_str().unwrap();
    let (status,user)=app.send("POST","/api/v1/users",json!({"username":"http-rpc-reader","displayName":"RPC reader","password":PASSWORD,"roleIds":[role_id]}),Some(admin)).await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    let (status, auth) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"http-rpc-reader","password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/api/v1/mcp/server",
        listener.local_addr().unwrap()
    );
    let router = app.router.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", auth["data"]["accessToken"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    let client = connect(
        HttpSpec {
            endpoint,
            client: http,
            headers,
        },
        Limits {
            startup_timeout: Duration::from_secs(3),
            request_timeout: Duration::from_secs(3),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tools = client.tools(CancellationToken::new()).await.unwrap();
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().all(|tool| {
        ["parameters.list", "parameters.get"].contains(&tool["name"].as_str().unwrap())
    }));
    let listed = client
        .call("parameters.list", json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(listed.value["structuredContent"]["meta"]["total"], 0);
    let denied=client.call("parameters.create",json!({"input":{"name":"must not create","key":"mcp.http.denied","valueType":"boolean","value":true}}),CancellationToken::new()).await.unwrap();
    assert_eq!(denied.value["isError"], true);
    assert_eq!(
        denied.value["structuredContent"]["error"]["code"],
        "FORBIDDEN"
    );
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"_id":ObjectId::parse_str(role_id).unwrap()},
            doc! {"$set":{"permissions":["mcp.read","mcp.invoke"]}},
        )
        .await
        .unwrap();
    assert!(
        client
            .tools(CancellationToken::new())
            .await
            .unwrap()
            .is_empty()
    );
    let denied = client
        .call("parameters.list", json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        denied.value["structuredContent"]["error"]["code"],
        "FORBIDDEN"
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.close().await;
    server.abort();
    let _ = server.await;
    app.cleanup().await;
}
