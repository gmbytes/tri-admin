mod support;
use admin_api::{build_router, config::Config, state::AppState};
use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::time::Duration;
use support::{PASSWORD, TestApp};
use tempfile::TempDir;
fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
async fn fixture() -> (TestApp, AppState, TempDir) {
    fixture_mode("normal").await
}
async fn fixture_mode(mode: &str) -> (TestApp, AppState, TempDir) {
    let mut app = TestApp::new().await;
    let temp = TempDir::new().unwrap();
    let mut environment = json!({"TRI_FIXTURE_ALLOWED":"sealed-result-marker"});
    #[cfg(windows)]
    {
        environment["SystemRoot"] = json!(std::env::var("SystemRoot").unwrap());
    }
    let capability = json!({"formatVersion":1,"programs":[{"id":"records","label":"受控记录服务","program":env!("CARGO_BIN_EXE_tri-admin-mcp-test-peer"),"arguments":[mode,temp.path().to_string_lossy()],"workingDirectory":temp.path(),"environment":environment,"allowedTools":["increment","inspect","effect_then_wait","crash","rpc_error","large_result"],"startupTimeoutMs":2000,"requestTimeoutMs":300,"stopTimeoutMs":3000}]});
    let path = temp.path().join("programs.json");
    std::fs::write(&path, serde_json::to_vec(&capability).unwrap()).unwrap();
    let config = Config::from_lookup(|name| match name {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_MCP_PROGRAMS_FILE" => Some(path.to_string_lossy().into()),
        _ => None,
    })
    .unwrap();
    let state = AppState::new(config, Some(app.database.clone()));
    app.router = build_router(state.clone());
    (app, state, temp)
}

#[tokio::test]
async fn stop_interrupts_startup_and_revoked_authority_cannot_publish_running() {
    let (app, _state, temp) = fixture_mode("discovery_delay").await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let row = connection(&app, owner, "启动停止").await;
    let id = row["id"].as_str().unwrap();
    let started = action(&app, owner, id, 1, "start").await;
    let stopped = action(&app, owner, id, 2, "stop").await;
    assert_eq!(
        settled(&app, owner, stopped["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    assert_ne!(
        settled(&app, owner, started["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let actor = account(
        &app,
        owner,
        "startup-writer",
        vec!["mcp.read", "mcp.write"],
        "self",
    )
    .await;
    let row = connection(&app, &actor, "撤权启动").await;
    let id = row["id"].as_str().unwrap();
    let _ = std::fs::remove_file(temp.path().join("methods.txt"));
    let started = action(&app, &actor, id, 1, "start").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if std::fs::read_to_string(temp.path().join("methods.txt"))
                .unwrap_or_default()
                .contains("server/discover")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let role = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"startup-writer"})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("_id")
        .unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{}", role.to_hex()),
            json!({"version":1,"permissions":["mcp.read"]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    let finished = settled(&app, owner, started["id"].as_str().unwrap()).await;
    assert_ne!(finished["status"], "succeeded", "{finished}");
    let instance=app.database.collection::<Document>("mcp_instances").find_one(doc!{"_id":mongodb::bson::oid::ObjectId::parse_str(finished["instanceId"].as_str().unwrap()).unwrap()}).await.unwrap().unwrap();
    assert!(instance.get_bool("exitConfirmed").unwrap());
    app.cleanup().await;
}

#[tokio::test]
async fn token_scope_cancel_and_large_encrypted_results_are_real() {
    let (app, _state, _temp) = fixture().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let(_,issued)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid(),"name":"MCP automation","scopes":["mcp.read","mcp.write","mcp.invoke"]}),Some(owner)).await;
    let token = issued["data"]["secret"].as_str().unwrap();
    let row = connection(&app, token, "自动化连接").await;
    let id = row["id"].as_str().unwrap();
    let start = action(&app, token, id, 1, "start").await;
    settled(&app, token, start["id"].as_str().unwrap()).await;
    let (_, limited) = app
        .send(
            "POST",
            "/api/v1/api-tokens",
            json!({"requestId":uuid(),"name":"MCP read","scopes":["mcp.read"]}),
            Some(owner),
        )
        .await;
    let reader = limited["data"]["secret"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/calls"),
            json!({"requestId":uuid(),"version":2,"name":"increment","arguments":{}}),
            Some(reader)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let running = call(&app, token, id, 2, "effect_then_wait", &uuid()).await;
    let call_id = running["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/mcp/actions/{call_id}/cancel"),
            json!({"requestId":uuid()}),
            Some(token)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let cancelled = settled(&app, token, call_id).await;
    assert!(
        ["unknown", "cancelled"].contains(&cancelled["status"].as_str().unwrap()),
        "{cancelled}"
    );
    let large = call(&app, token, id, 2, "large_result", &uuid()).await;
    let large_id = large["id"].as_str().unwrap();
    assert_eq!(settled(&app, token, large_id).await["status"], "succeeded");
    let payload = result(&app, token, large_id).await;
    assert_eq!(payload["content"][0]["text"].as_str().unwrap().len(), 50000);
    let stop = action(&app, token, id, 2, "stop").await;
    settled(&app, token, stop["id"].as_str().unwrap()).await;
    app.cleanup().await;
}

#[tokio::test]
async fn crash_is_observed_without_rebinding_old_call_or_instance_ids() {
    let (app, _state, _temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = connection(&app, token, "崩溃观测").await;
    let id = row["id"].as_str().unwrap();
    let start = action(&app, token, id, 1, "start").await;
    let first = settled(&app, token, start["id"].as_str().unwrap()).await;
    let request = uuid();
    let crashing = call(&app, token, id, 2, "crash", &request).await;
    assert_eq!(
        settled(&app, token, crashing["id"].as_str().unwrap()).await["status"],
        "unknown"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (_, row) = app
                .send(
                    "GET",
                    &format!("/api/v1/mcp/connections/{id}"),
                    Value::Null,
                    Some(token),
                )
                .await;
            if row["data"]["runtime"]["exitConfirmed"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let second = action(&app, token, id, 2, "start").await;
    let second = settled(&app, token, second["id"].as_str().unwrap()).await;
    assert_ne!(first["instanceId"], second["instanceId"]);
    assert_eq!(
        call(&app, token, id, 2, "crash", &request).await["id"],
        crashing["id"]
    );
    let stop = action(&app, token, id, 3, "stop").await;
    settled(&app, token, stop["id"].as_str().unwrap()).await;
    app.cleanup().await;
}

#[tokio::test]
async fn expired_service_ownership_is_orphaned_and_needs_explicit_session_confirmation() {
    use admin_api::modules::mcp::service;
    let (app, state, _temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = connection(&app, token, "遗留所有权").await;
    let id = row["id"].as_str().unwrap();
    let started = action(&app, token, id, 1, "start").await;
    let started = settled(&app, token, started["id"].as_str().unwrap()).await;
    let instance =
        mongodb::bson::oid::ObjectId::parse_str(started["instanceId"].as_str().unwrap()).unwrap();
    service::shutdown(&state).await;
    // Reproduce a crash before the observed exit could be committed, after the real fixture is stopped.
    let past = mongodb::bson::DateTime::from_millis(1);
    app.database
        .collection::<Document>("mcp_instances")
        .update_one(
            doc! {"_id":instance},
            doc! {"$set":{"state":"running","exitConfirmed":false,"leaseUntil":past}},
        )
        .await
        .unwrap();
    app.database.collection::<Document>("mcp_connections").update_one(doc!{"_id":mongodb::bson::oid::ObjectId::parse_str(id).unwrap()},doc!{"$set":{"runtime.state":"running","runtime.exitConfirmed":false,"runtime.leaseUntil":past,"controlActionId":null}}).await.unwrap();
    let replacement_state = AppState::new((*state.config).clone(), Some(app.database.clone()));
    let replacement = TestApp {
        router: build_router(replacement_state.clone()),
        database: app.database.clone(),
    };
    let report = service::maintenance_once(&replacement_state).await.unwrap();
    assert_eq!(report.orphaned, 1);
    let (_, row) = replacement
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(row["data"]["runtime"]["state"], "orphaned");
    assert_eq!(
        replacement
            .send(
                "POST",
                &format!("/api/v1/mcp/connections/{id}/actions"),
                json!({"requestId":uuid(),"version":2,"kind":"start"}),
                Some(token)
            )
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(replacement.send("POST",&format!("/api/v1/mcp/connections/{id}/confirm-orphan"),json!({"requestId":uuid(),"version":2,"confirmed":true,"reason":"已确认原程序清理"}),Some(token)).await.0,StatusCode::OK);
    let new = action(&replacement, token, id, 3, "start").await;
    assert_eq!(
        settled(&replacement, token, new["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let stop = action(&replacement, token, id, 4, "stop").await;
    settled(&replacement, token, stop["id"].as_str().unwrap()).await;
    replacement.cleanup().await;
}

#[tokio::test]
async fn old_request_receipts_survive_missing_deployment_capabilities_and_result_keys() {
    let (app, state, _temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = connection(&app, token, "稳定回执").await;
    let id = row["id"].as_str().unwrap();
    let start = action(&app, token, id, 1, "start").await;
    settled(&app, token, start["id"].as_str().unwrap()).await;
    let request = uuid();
    let called = call(&app, token, id, 2, "increment", &request).await;
    settled(&app, token, called["id"].as_str().unwrap()).await;
    let stop = action(&app, token, id, 2, "stop").await;
    settled(&app, token, stop["id"].as_str().unwrap()).await;
    let config = Config::from_lookup(|key| {
        if key == "TRI_JWT_SIGNING_KEY" {
            Some(support::KEY.into())
        } else {
            None
        }
    })
    .unwrap();
    let replacement = TestApp {
        router: build_router(AppState::new(config, Some(app.database.clone()))),
        database: app.database.clone(),
    };
    let(status,replayed)=replacement.send("POST","/api/v1/mcp/connections",json!({"requestId":row["requestId"],"name":row["name"],"transport":row["transport"],"enabled":true}),Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["data"]["id"], row["id"]);
    let (status, replayed) = replacement
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/calls"),
            json!({"requestId":request,"version":2,"name":"increment","arguments":{}}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["data"]["id"], called["id"]);
    assert_eq!(
        replacement
            .send(
                "GET",
                &format!(
                    "/api/v1/mcp/actions/{}/result",
                    called["id"].as_str().unwrap()
                ),
                Value::Null,
                Some(token)
            )
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(state);
    replacement.cleanup().await;
}

#[tokio::test]
async fn real_main_http_serves_mcp_and_trace_logs_never_include_sdk_payloads() {
    use std::{
        fs::File,
        process::{Command, Stdio},
    };
    struct Guard(std::process::Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let (app, _state, temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let environment = dotenvy::from_path_iter(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env"),
    )
    .unwrap()
    .map(Result::unwrap)
    .collect::<std::collections::HashMap<_, _>>();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let out = temp.path().join("main.out");
    let err = temp.path().join("main.err");
    let mut command = Command::new(env!("CARGO_BIN_EXE_admin-api"));
    command
        .arg("serve")
        .current_dir(temp.path())
        .env_clear()
        .env(
            "TRI_MONGODB_URI",
            environment.get("TRI_MONGODB_URI").unwrap(),
        )
        .env("TRI_MONGODB_DATABASE", app.database.name())
        .env("TRI_JWT_SIGNING_KEY", support::KEY)
        .env("TRI_SETTINGS_KEYS", format!("fixture:{}", "11".repeat(32)))
        .env("TRI_SETTINGS_ACTIVE_KEY", "fixture")
        .env("TRI_MCP_PROGRAMS_FILE", temp.path().join("programs.json"))
        .env("TRI_STORAGE_ROOT", temp.path().join("storage"))
        .env("TRI_ADMIN_BIND", address.to_string())
        .env("RUST_LOG", "trace")
        .stdin(Stdio::null())
        .stdout(Stdio::from(File::create(&out).unwrap()))
        .stderr(Stdio::from(File::create(&err).unwrap()));
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
        command.env("SystemRoot", std::env::var_os("SystemRoot").unwrap());
    }
    let mut server = Guard(command.spawn().unwrap());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let base = format!("http://{address}");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                server.0.try_wait().unwrap().is_none(),
                "main stopped before ready"
            );
            if client
                .get(format!("{base}/health/ready"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let response=client.post(format!("{base}/api/v1/mcp/connections")).bearer_auth(token).header("content-type","application/json").body(serde_json::to_vec(&json!({"requestId":uuid(),"name":"真实主服务","transport":{"kind":"stdio","capabilityId":"records"},"enabled":true})).unwrap()).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let row: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let id = row["data"]["id"].as_str().unwrap();
    let response = client
        .post(format!("{base}/api/v1/mcp/connections/{id}/actions"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&json!({"requestId":uuid(),"version":1,"kind":"start"})).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let started: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(
        settled(&app, token, started["data"]["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    for tool in ["inspect", "rpc_error"] {
        let response=client.post(format!("{base}/api/v1/mcp/connections/{id}/calls")).bearer_auth(token).header("content-type","application/json").body(serde_json::to_vec(&json!({"requestId":uuid(),"version":2,"name":tool,"arguments":{"private":"private-request-marker"}})).unwrap()).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let called: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        settled(&app, token, called["data"]["id"].as_str().unwrap()).await;
    }
    let response = client
        .post(format!("{base}/api/v1/mcp/connections/{id}/actions"))
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&json!({"requestId":uuid(),"version":2,"kind":"stop"})).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let stopped: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(
        settled(&app, token, stopped["data"]["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    server.0.kill().unwrap();
    server.0.wait().unwrap();
    let logs = format!(
        "{}{}",
        std::fs::read_to_string(out).unwrap(),
        std::fs::read_to_string(err).unwrap()
    );
    for marker in [
        "private-request-marker",
        "sealed-result-marker",
        "SYNTHETIC-REMOTE-DETAIL",
    ] {
        assert!(
            !logs.contains(marker),
            "sensitive SDK payload appeared in trace logs"
        );
    }
    app.cleanup().await;
}
async fn connection(app: &TestApp, token: &str, name: &str) -> Value {
    let(status,row)=app.send("POST","/api/v1/mcp/connections",json!({"requestId":uuid(),"name":name,"transport":{"kind":"stdio","capabilityId":"records"},"enabled":true}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{row}");
    row["data"].clone()
}
async fn action(app: &TestApp, token: &str, id: &str, version: i64, kind: &str) -> Value {
    let (status, row) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            json!({"requestId":uuid(),"version":version,"kind":kind}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{row}");
    row["data"].clone()
}
async fn settled(app: &TestApp, token: &str, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (status, row) = app
                .send(
                    "GET",
                    &format!("/api/v1/mcp/actions/{id}"),
                    Value::Null,
                    Some(token),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{row}");
            if ["succeeded", "failed", "unknown", "cancelled"]
                .contains(&row["data"]["status"].as_str().unwrap_or(""))
            {
                return row["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}
async fn call(
    app: &TestApp,
    token: &str,
    id: &str,
    version: i64,
    name: &str,
    request: &str,
) -> Value {
    let (status, row) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/calls"),
            json!({"requestId":request,"version":version,"name":name,"arguments":{}}),
            Some(token),
        )
        .await;
    assert!(
        [StatusCode::ACCEPTED, StatusCode::OK].contains(&status),
        "{status}: {row}"
    );
    row["data"].clone()
}
async fn result(app: &TestApp, token: &str, id: &str) -> Value {
    let (status, row) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/actions/{id}/result"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{row}");
    serde_json::from_str(row["data"]["content"].as_str().unwrap()).unwrap()
}
async fn account(
    app: &TestApp,
    owner: &str,
    name: &str,
    permissions: Vec<&str>,
    scope: &str,
) -> String {
    let (status, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":name,"slug":name,"permissions":permissions,"dataScope":scope}),
            Some(owner),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await.0,StatusCode::CREATED);
    let (_, signed) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    signed["data"]["accessToken"].as_str().unwrap().into()
}
#[tokio::test]
async fn approved_capabilities_lifecycle_and_calls_have_durable_identity() {
    let (app, _state, _temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (status, caps) = app
        .send("GET", "/api/v1/mcp/capabilities", Value::Null, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    let public = caps.to_string();
    assert!(!public.contains("sealed-result-marker"));
    assert!(!public.contains("workingDirectory"));
    assert!(!public.contains(".exe"));
    let created = connection(&app, token, "记录服务").await;
    let id = created["id"].as_str().unwrap();
    let request = uuid();
    let intent = json!({"requestId":request,"version":1,"kind":"start"});
    let action_path = format!("/api/v1/mcp/connections/{id}/actions");
    let (a, b) = tokio::join!(
        app.send("POST", &action_path, intent.clone(), Some(token)),
        app.send("POST", &action_path, intent.clone(), Some(token))
    );
    assert!([a.0, b.0].contains(&StatusCode::ACCEPTED));
    assert!([a.0, b.0].contains(&StatusCode::OK));
    assert_eq!(a.1["data"]["id"], b.1["data"]["id"]);
    let started = settled(&app, token, a.1["data"]["id"].as_str().unwrap()).await;
    assert_eq!(started["status"], "succeeded", "{started}");
    let (_, current) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(current["data"]["version"], 2);
    assert_eq!(current["data"]["runtime"]["state"], "running");
    let (_, tools) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}/tools"),
            Value::Null,
            Some(token),
        )
        .await;
    assert!(
        tools["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "increment")
    );
    let request = uuid();
    let first = call(&app, token, id, 2, "increment", &request).await;
    let same = call(&app, token, id, 2, "increment", &request).await;
    assert_eq!(first["id"], same["id"]);
    let finished = settled(&app, token, first["id"].as_str().unwrap()).await;
    assert_eq!(finished["status"], "succeeded", "{finished}");
    assert_eq!(
        result(&app, token, first["id"].as_str().unwrap()).await["structuredContent"]["value"],
        1
    );
    let inspected = call(&app, token, id, 2, "inspect", &uuid()).await;
    settled(&app, token, inspected["id"].as_str().unwrap()).await;
    assert_eq!(
        result(&app, token, inspected["id"].as_str().unwrap()).await["structuredContent"]["value"],
        1
    );
    let encrypted = app
        .database
        .collection::<Document>("mcp_results")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!encrypted.to_string().contains("sealed-result-marker"));
    let restart = action(&app, token, id, 2, "restart").await;
    let restarted = settled(&app, token, restart["id"].as_str().unwrap()).await;
    assert_eq!(restarted["status"], "succeeded", "{restarted}");
    let inspect = call(&app, token, id, 3, "inspect", &uuid()).await;
    settled(&app, token, inspect["id"].as_str().unwrap()).await;
    assert_eq!(
        result(&app, token, inspect["id"].as_str().unwrap()).await["structuredContent"]["value"],
        0
    );
    let stop = action(&app, token, id, 3, "stop").await;
    assert_eq!(
        settled(&app, token, stop["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    assert_eq!(
        app.database
            .collection::<Document>("mcp_instances")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    app.cleanup().await;
}
#[tokio::test]
async fn ownership_readonly_and_token_scope_never_grant_process_or_tool_authority() {
    let (app, _state, _temp) = fixture().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let alpha = account(
        &app,
        owner,
        "mcp-alpha",
        vec!["mcp.read", "mcp.write", "mcp.invoke"],
        "self",
    )
    .await;
    let bravo = account(
        &app,
        owner,
        "mcp-bravo",
        vec!["mcp.read", "mcp.write", "mcp.invoke"],
        "self",
    )
    .await;
    let reader = account(&app, owner, "mcp-reader", vec!["mcp.read"], "all").await;
    let row = connection(&app, &alpha, "私有服务").await;
    let id = row["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(&bravo)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send("GET", "/api/v1/mcp/connections", Value::Null, Some(&bravo))
            .await
            .1["meta"]["total"],
        0
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            json!({"requestId":uuid(),"version":1,"kind":"start"}),
            Some(&reader)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let started = action(&app, &alpha, id, 1, "start").await;
    settled(&app, &alpha, started["id"].as_str().unwrap()).await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/calls"),
            json!({"requestId":uuid(),"version":2,"name":"inspect","arguments":{}}),
            Some(&reader)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let actor = call(&app, &alpha, id, 2, "inspect", &uuid()).await;
    settled(&app, &alpha, actor["id"].as_str().unwrap()).await;
    assert_eq!(
        app.send(
            "GET",
            &format!(
                "/api/v1/mcp/actions/{}/result",
                actor["id"].as_str().unwrap()
            ),
            Value::Null,
            Some(owner)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let stopped = action(&app, &alpha, id, 2, "stop").await;
    settled(&app, &alpha, stopped["id"].as_str().unwrap()).await;
    app.cleanup().await;
}
#[tokio::test]
async fn timed_out_calls_stay_unknown_and_replay_does_not_repeat_the_effect() {
    let (app, _state, _temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = connection(&app, token, "调用回执").await;
    let id = row["id"].as_str().unwrap();
    let start = action(&app, token, id, 1, "start").await;
    settled(&app, token, start["id"].as_str().unwrap()).await;
    let request = uuid();
    let timed = call(&app, token, id, 2, "effect_then_wait", &request).await;
    let unknown = settled(&app, token, timed["id"].as_str().unwrap()).await;
    assert_eq!(unknown["status"], "unknown", "{unknown}");
    assert_eq!(
        call(&app, token, id, 2, "effect_then_wait", &request).await["id"],
        timed["id"]
    );
    let inspect = call(&app, token, id, 2, "inspect", &uuid()).await;
    settled(&app, token, inspect["id"].as_str().unwrap()).await;
    assert_eq!(
        result(&app, token, inspect["id"].as_str().unwrap()).await["structuredContent"]["value"],
        1
    );
    let stopped = action(&app, token, id, 2, "stop").await;
    settled(&app, token, stopped["id"].as_str().unwrap()).await;
    app.cleanup().await;
}
#[tokio::test]
async fn invalid_config_inputs_and_failed_intent_audit_cannot_spawn_a_process() {
    let (app, _state, temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut malicious = json!({"requestId":uuid(),"name":"禁止命令","transport":{"kind":"stdio","capabilityId":"records"},"enabled":true});
    malicious["program"] = json!("unapproved-program");
    assert_eq!(
        app.send("POST", "/api/v1/mcp/connections", malicious, Some(token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let row = connection(&app, token, "审计门禁").await;
    let id = row["id"].as_str().unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"mcp.start"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            json!({"requestId":uuid(),"version":1,"kind":"start"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("mcp_instances")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert!(!temp.path().join("methods.txt").exists());
    app.cleanup().await;
}
