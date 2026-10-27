mod support;
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use support::{PASSWORD, TestApp};
async fn profile(app: &TestApp, token: &str, url: &str) -> String {
    let(status,body)=app.send("POST","/api/v1/settings/profiles",json!({"name":"UI support HTTP","code":"ui.support.http","enabled":true,"configuration":{"kind":"http","baseUrl":url}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"]["id"].as_str().unwrap().into()
}
async fn create(app: &TestApp, token: &str, profile: &str, enabled: bool) -> String {
    let(status,body)=app.send("POST","/api/v1/mcp/connections",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"UI support connection","enabled":enabled,"transport":{"kind":"http","profileId":profile,"profileVersion":1,"allowedTools":["old_tool","new_tool"]}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"]["id"].as_str().unwrap().into()
}
async fn terminal(app: &TestApp, token: &str, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (status, body) = app
            .send(
                "GET",
                &format!("/api/v1/mcp/actions/{id}"),
                Value::Null,
                Some(token),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let row = body["data"].clone();
        if !["pending", "running"].contains(&row["status"].as_str().unwrap()) {
            return row;
        }
        assert!(Instant::now() < deadline, "{row}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn explicit_refresh_fetches_current_tools_once_without_changing_config_or_instance() {
    let changed = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let flag = changed.clone();
    let count = calls.clone();
    let router=axum::Router::new().route("/mcp",axum::routing::post(move|request:axum::extract::Request|{let flag=flag.clone();let count=count.clone();async move{
        let value:Value=serde_json::from_slice(&axum::body::to_bytes(request.into_body(),128*1024).await.unwrap()).unwrap();
        let result=if value["method"]=="server/discover"{json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"})}else{count.fetch_add(1,Ordering::SeqCst);json!({"resultType":"complete","tools":[{"name":if flag.load(Ordering::SeqCst){"new_tool"}else{"old_tool"},"inputSchema":{"type":"object"}}],"ttlMs":0,"cacheScope":"private"})};
        axum::Json(json!({"jsonrpc":"2.0","id":value["id"],"result":result}))
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, &endpoint).await;
    let id = create(&app, token, &profile, true).await;
    let (status, start) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"kind":"start"}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{start}");
    assert_eq!(
        terminal(&app, token, start["data"]["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let (_, before) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    let instance = before["data"]["runtime"]["id"].clone();
    changed.store(true, Ordering::SeqCst);
    let (_, old) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}/tools"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(old["data"][0]["name"], "old_tool");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let request = uuid::Uuid::new_v4().to_string();
    let input = json!({"requestId":request,"version":2,"kind":"refresh"});
    let (status, refresh) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            input.clone(),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{refresh}");
    assert_eq!(
        terminal(&app, token, refresh["data"]["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let (_, tools) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}/tools"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(tools["data"][0]["name"], "new_tool");
    let (_, after) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(after["data"]["version"], 2);
    assert_eq!(after["data"]["runtime"]["id"], instance);
    let (status, replay) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            input,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["data"]["id"], refresh["data"]["id"]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let (status, stop) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":2,"kind":"stop"}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{stop}");
    let _ = terminal(&app, token, stop["data"]["id"].as_str().unwrap()).await;
    server.abort();
    let _ = server.await;
    app.cleanup().await;
}

#[tokio::test]
async fn deleted_connections_keep_scoped_metadata_and_history_without_reappearing_as_active() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, "http://127.0.0.1:9/mcp").await;
    let id = create(&app, token, &profile, false).await;
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/mcp/connections/{id}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (status, deleted) = app
        .send(
            "GET",
            "/api/v1/mcp/connections?status=deleted",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["meta"]["total"], 1);
    assert_eq!(deleted["data"][0]["id"], id);
    assert!(deleted["data"][0]["deletedAt"].is_string());
    let (_, active) = app
        .send("GET", "/api/v1/mcp/connections", Value::Null, Some(token))
        .await;
    assert_eq!(active["meta"]["total"], 0);
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let(status,role)=app.send("POST","/api/v1/roles",json!({"name":"MCP self history","slug":"mcp-self-history","permissions":["mcp.read"],"dataScope":"self"}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let(status,user)=app.send("POST","/api/v1/users",json!({"username":"mcp-self-history","displayName":"Self history","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    let (_, auth) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"mcp-self-history","password":PASSWORD}),
            None,
        )
        .await;
    let outsider = auth["data"]["accessToken"].as_str().unwrap();
    let (_, hidden) = app
        .send(
            "GET",
            "/api/v1/mcp/connections?status=deleted",
            Value::Null,
            Some(outsider),
        )
        .await;
    assert_eq!(hidden["meta"]["total"], 0);
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            Value::Null,
            Some(outsider)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.cleanup().await;
}
