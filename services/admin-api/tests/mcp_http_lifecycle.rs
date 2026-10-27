#[path = "../../../crates/mcp-runtime/tests/support/http_peer.rs"]
#[allow(dead_code)]
mod http_peer;
mod support;
use axum::http::StatusCode;
use http_peer::{Mode, Peer};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use support::TestApp;

async fn create(app: &TestApp, token: &str, peer: &Peer) -> String {
    let (status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"HTTP lifecycle","code":"mcp.http.lifecycle","enabled":true,"configuration":{"kind":"http","baseUrl":peer.url},"secret":{"kind":"http","headerName":"authorization","value":"Bearer synthetic-lifecycle-secret"}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    let (status,connection)=app.send("POST","/api/v1/mcp/connections",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"HTTP lifecycle","enabled":true,"transport":{"kind":"http","profileId":profile["data"]["id"],"profileVersion":1,"allowedTools":["increment","hold"]}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{connection}");
    assert!(peer.seen.lock().unwrap().is_empty());
    connection["data"]["id"].as_str().unwrap().into()
}
async fn receipt(app: &TestApp, token: &str, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(8);
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
        let action = body["data"].clone();
        if !["pending", "running"].contains(&action["status"].as_str().unwrap()) {
            return action;
        }
        assert!(Instant::now() < deadline, "{action}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn control(app: &TestApp, token: &str, id: &str, version: i64, kind: &str) -> Value {
    let (status, body) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":version,"kind":kind}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    receipt(app, token, body["data"]["id"].as_str().unwrap()).await
}
async fn connection(app: &TestApp, token: &str, id: &str) -> Value {
    let (status, body) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}

#[tokio::test]
async fn actual_http_lifecycle_restarts_without_pid_and_replays_only_original_receipts() {
    let peer = Peer::start(Mode::Json).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = create(&app, token, &peer).await;
    assert_eq!(
        control(&app, token, &id, 1, "start").await["status"],
        "succeeded"
    );
    let first = connection(&app, token, &id).await;
    assert_eq!(first["runtime"]["transportKind"], "http");
    assert_eq!(first["runtime"]["pid"], Value::Null);
    assert_eq!(first["runtime"]["releaseConfirmed"], false);
    let instance = first["runtime"]["id"].as_str().unwrap().to_owned();
    let request = uuid::Uuid::new_v4().to_string();
    let input = json!({"requestId":request,"version":2,"name":"increment","arguments":{}});
    let (status, body) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/calls"),
            input.clone(),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let action_id = body["data"]["id"].as_str().unwrap();
    assert_eq!(receipt(&app, token, action_id).await["status"], "succeeded");
    let (status, result) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/actions/{action_id}/result"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let result: Value = serde_json::from_str(result["data"]["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["structuredContent"]["value"], 1);
    assert_eq!(
        control(&app, token, &id, 2, "restart").await["status"],
        "succeeded"
    );
    let next = connection(&app, token, &id).await;
    assert_ne!(next["runtime"]["id"], instance);
    assert_eq!(next["runtime"]["pid"], Value::Null);
    let (status, replay) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/calls"),
            input,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["data"]["id"], action_id);
    assert_eq!(peer.effects.load(Ordering::SeqCst), 1);
    assert_eq!(
        control(&app, token, &id, 3, "stop").await["status"],
        "succeeded"
    );
    let stopped = connection(&app, token, &id).await;
    assert_eq!(stopped["runtime"]["releaseConfirmed"], true);
    assert_eq!(stopped["runtime"]["state"], "stopped");
    let old = app
        .database
        .collection::<Document>("mcp_instances")
        .find_one(doc! {"_id":ObjectId::parse_str(&instance).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert!(old.get_bool("releaseConfirmed").unwrap());
    assert!(old.get("pid").unwrap().as_null().is_some());
    {
        let seen = peer.seen.lock().unwrap();
        assert!(
            seen.iter()
                .all(|row| row.method == "POST" && row.rpc != "initialize")
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn expired_http_ownership_is_fenced_without_manual_process_confirmation() {
    let peer = Peer::start(Mode::Json).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = create(&app, token, &peer).await;
    // Durable residue from a lost HTTP owner; there is deliberately no local handle.
    let instance = ObjectId::new();
    let actor = ObjectId::parse_str(auth["user"]["id"].as_str().unwrap()).unwrap();
    let past = DateTime::from_millis(DateTime::now().timestamp_millis() - 60_000);
    let service = uuid::Uuid::new_v4().to_string();
    app.database.collection::<Document>("mcp_instances").insert_one(doc!{"_id":instance,"connectionId":ObjectId::parse_str(&id).unwrap(),"ownerId":actor,"actorId":actor,"serviceId":&service,"state":"running","transportKind":"http","pid":null,"releaseConfirmed":false,"leaseUntil":past,"toolsJson":"[]","errorCode":null,"createdAt":past,"updatedAt":past}).await.unwrap();
    app.database.collection::<Document>("mcp_connections").update_one(doc!{"_id":ObjectId::parse_str(&id).unwrap()},doc!{"$set":{"version":2i64,"runtime":{"id":instance,"serviceId":&service,"state":"running","transportKind":"http","pid":null,"releaseConfirmed":false,"leaseUntil":past,"updatedAt":past,"errorCode":null}}}).await.unwrap();
    let pending = ObjectId::new();
    app.database.collection::<Document>("mcp_actions").insert_one(doc!{"_id":pending,"actorId":actor,"requestId":uuid::Uuid::new_v4().to_string(),"instanceId":instance,"status":"running"}).await.unwrap();
    let config = admin_api::config::Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        _ => None,
    })
    .unwrap();
    let state = admin_api::state::AppState::new(config, Some(app.database.clone()));
    let (status,_)=app.send("POST",&format!("/api/v1/mcp/connections/{id}/confirm-orphan"),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":2,"confirmed":true,"reason":"HTTP is not a process cleanup statement"}),Some(token)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let report = admin_api::modules::mcp::service::maintenance_once(&state)
        .await
        .unwrap();
    assert_eq!(report.scanned, 1);
    let pending = app
        .database
        .collection::<Document>("mcp_actions")
        .find_one(doc! {"_id":pending})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.get_str("status").unwrap(), "unknown");
    assert!(peer.seen.lock().unwrap().is_empty());
    let mut replacement = app;
    replacement.router = admin_api::build_router(state);
    let row = connection(&replacement, token, &id).await;
    assert_eq!(row["runtime"]["state"], "disconnected");
    assert_eq!(row["runtime"]["releaseConfirmed"], true);
    assert_eq!(row["runtime"]["pid"], Value::Null);
    assert_eq!(
        control(&replacement, token, &id, 2, "start").await["status"],
        "succeeded"
    );
    assert_ne!(
        connection(&replacement, token, &id).await["runtime"]["id"],
        instance.to_hex()
    );
    assert_eq!(
        control(&replacement, token, &id, 3, "stop").await["status"],
        "succeeded"
    );
    replacement.cleanup().await;
}

#[tokio::test]
async fn replaced_http_ownership_closes_only_the_old_owned_session() {
    let peer = Peer::start(Mode::Json).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = create(&app, token, &peer).await;
    assert_eq!(
        control(&app, token, &id, 1, "start").await["status"],
        "succeeded"
    );
    let before = connection(&app, token, &id).await;
    let old = ObjectId::parse_str(before["runtime"]["id"].as_str().unwrap()).unwrap();
    let replacement = ObjectId::new();
    let now = DateTime::now();
    app.database.collection::<Document>("mcp_connections").update_one(doc!{"_id":ObjectId::parse_str(&id).unwrap()},doc!{"$set":{"version":3i64,"runtime":{"id":replacement,"serviceId":"replacement-owner","state":"running","transportKind":"http","pid":null,"releaseConfirmed":false,"leaseUntil":DateTime::from_millis(now.timestamp_millis()+30_000),"updatedAt":now,"errorCode":null}}}).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let row = app
            .database
            .collection::<Document>("mcp_instances")
            .find_one(doc! {"_id":old})
            .await
            .unwrap()
            .unwrap();
        if row.get_bool("releaseConfirmed").unwrap() {
            break;
        }
        assert!(Instant::now() < deadline, "old HTTP owner was not released");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let current = app
        .database
        .collection::<Document>("mcp_connections")
        .find_one(doc! {"_id":ObjectId::parse_str(&id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current
            .get_document("runtime")
            .unwrap()
            .get_object_id("id")
            .unwrap(),
        replacement
    );
    assert_eq!(peer.effects.load(Ordering::SeqCst), 0);
    app.cleanup().await;
}

#[tokio::test]
async fn cancelled_calls_and_changed_profiles_preserve_one_remote_effect() {
    let peer = Peer::start(Mode::Json).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = create(&app, token, &peer).await;
    assert_eq!(
        control(&app, token, &id, 1, "start").await["status"],
        "succeeded"
    );
    let request = uuid::Uuid::new_v4().to_string();
    let input = json!({"requestId":request,"version":2,"name":"hold","arguments":{}});
    let (status, body) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/calls"),
            input.clone(),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let action = body["data"]["id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while peer.effects.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (status, body) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/actions/{action}/cancel"),
            json!({"requestId":uuid::Uuid::new_v4().to_string()}),
            Some(token),
        )
        .await;
    assert!(
        matches!(status, StatusCode::ACCEPTED | StatusCode::OK),
        "{body}"
    );
    assert_eq!(receipt(&app, token, action).await["status"], "unknown");
    let (status, old) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/calls"),
            input,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{old}");
    assert_eq!(old["data"]["id"], action);
    assert_eq!(peer.effects.load(Ordering::SeqCst), 1);
    let row = connection(&app, token, &id).await;
    let profile = row["transport"]["profileId"].as_str().unwrap();
    let (status, body) = app
        .send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{profile}"),
            json!({"version":1,"name":"credential reference changed"}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let(status,_)=app.send("POST",&format!("/api/v1/mcp/connections/{id}/calls"),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":2,"name":"increment","arguments":{}}),Some(token)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let row = connection(&app, token, &id).await;
        if row["runtime"]["releaseConfirmed"] == true {
            assert_eq!(row["capabilityCurrent"], false);
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(peer.effects.load(Ordering::SeqCst), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn startup_redirects_cannot_forward_saved_credentials_to_another_target() {
    use axum::response::IntoResponse;
    let target = Peer::start(Mode::Json).await;
    let destination = target.url.clone();
    let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = requests.clone();
    let router = axum::Router::new().route(
        "/mcp",
        axum::routing::post(move |request: axum::extract::Request| {
            let destination = destination.clone();
            let observed = observed.clone();
            async move {
                let _ = axum::body::to_bytes(request.into_body(), 128 * 1024)
                    .await
                    .unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                (StatusCode::FOUND, [("location", destination)]).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let fake = Peer::start(Mode::Json).await;
    let id = create(&app, token, &fake).await;
    let row = connection(&app, token, &id).await;
    let profile = row["transport"]["profileId"].as_str().unwrap();
    let(status,body)=app.send("PATCH",&format!("/api/v1/settings/profiles/{profile}"),json!({"version":1,"configuration":{"kind":"http","baseUrl":endpoint},"secret":{"kind":"http","headerName":"authorization","value":"Bearer synthetic-redirect-secret"}}),Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let(status,body)=app.send("PATCH",&format!("/api/v1/mcp/connections/{id}"),json!({"version":1,"name":"HTTP lifecycle","enabled":true,"transport":{"kind":"http","profileId":profile,"profileVersion":2,"allowedTools":["increment"]}}),Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        control(&app, token, &id, 2, "start").await["status"],
        "failed"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(target.seen.lock().unwrap().is_empty());
    assert!(fake.seen.lock().unwrap().is_empty());
    server.abort();
    let _ = server.await;
    app.cleanup().await;
}

#[tokio::test]
async fn authority_or_profile_change_during_http_discovery_never_publishes_running() {
    for change_role in [true, false] {
        let app = TestApp::new().await;
        let admin = app.login().await;
        let admin_token = admin["accessToken"].as_str().unwrap();
        let (status,role)=app.send("POST","/api/v1/roles",json!({"name":"HTTP start operator","slug":"http-start-operator","permissions":["mcp.read","mcp.write","mcp.invoke","settings.write"],"dataScope":"self"}),Some(admin_token)).await;
        assert_eq!(status, StatusCode::CREATED, "{role}");
        let role_id = role["data"]["id"].as_str().unwrap();
        let (status,user)=app.send("POST","/api/v1/users",json!({"username":"http-start-operator","displayName":"HTTP operator","password":support::PASSWORD,"roleIds":[role_id]}),Some(admin_token)).await;
        assert_eq!(status, StatusCode::CREATED, "{user}");
        let (status, auth) = app
            .send(
                "POST",
                "/api/v1/auth/login",
                json!({"username":"http-start-operator","password":support::PASSWORD}),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let token = auth["data"]["accessToken"].as_str().unwrap();
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify = entered.clone();
        let gate = release.clone();
        let router=axum::Router::new().route("/mcp",axum::routing::post(move|request:axum::extract::Request|{let notify=notify.clone();let gate=gate.clone();async move{
            let body:Value=serde_json::from_slice(&axum::body::to_bytes(request.into_body(),128*1024).await.unwrap()).unwrap();
            let result=if body["method"]=="server/discover"{notify.notify_one();gate.notified().await;json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"})}else{json!({"resultType":"complete","tools":[],"ttlMs":0,"cacheScope":"private"})};
            axum::Json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        }}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let(status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"gated HTTP","code":"gated.http","enabled":true,"configuration":{"kind":"http","baseUrl":endpoint}}),Some(token)).await;
        assert_eq!(status, StatusCode::CREATED, "{profile}");
        let profile_id = profile["data"]["id"].as_str().unwrap();
        let(status,conn)=app.send("POST","/api/v1/mcp/connections",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"gated HTTP","enabled":true,"transport":{"kind":"http","profileId":profile_id,"profileVersion":1,"allowedTools":["increment"]}}),Some(token)).await;
        assert_eq!(status, StatusCode::CREATED, "{conn}");
        let id = conn["data"]["id"].as_str().unwrap();
        let (status, started) = app
            .send(
                "POST",
                &format!("/api/v1/mcp/connections/{id}/actions"),
                json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"kind":"start"}),
                Some(token),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{started}");
        tokio::time::timeout(Duration::from_secs(3), entered.notified())
            .await
            .unwrap();
        if change_role {
            let (status, body) = app
                .send(
                    "PATCH",
                    &format!("/api/v1/roles/{role_id}"),
                    json!({"version":1,"permissions":["mcp.read","mcp.invoke","settings.write"]}),
                    Some(admin_token),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        } else {
            let (status, body) = app
                .send(
                    "PATCH",
                    &format!("/api/v1/settings/profiles/{profile_id}"),
                    json!({"version":1,"name":"changed during discovery"}),
                    Some(token),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        release.notify_one();
        let result = receipt(&app, token, started["data"]["id"].as_str().unwrap()).await;
        assert!(
            matches!(result["status"].as_str(), Some("failed" | "cancelled")),
            "{result}"
        );
        let row = connection(&app, token, id).await;
        assert_ne!(row["runtime"]["state"], "running");
        assert_eq!(row["runtime"]["releaseConfirmed"], true);
        assert_eq!(row["runtime"]["pid"], Value::Null);
        server.abort();
        let _ = server.await;
        app.cleanup().await;
    }
}
