#[path = "support/mongo_faults.rs"]
#[allow(dead_code)]
mod mongo_faults;
mod support;
use admin_api::{build_router, config::Config, modules::ai::worker, state::AppState};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{Method, Request},
    response::{IntoResponse, Response},
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use support::TestApp;
#[derive(Clone)]
struct PeerState {
    mode: &'static str,
    seen: Arc<Mutex<Vec<Captured>>>,
}
struct Captured {
    method: String,
    path: String,
    body: Value,
}
struct Peer {
    url: String,
    state: PeerState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn response(status: &str, text: &str) -> Value {
    json!({"id":"resp_host_fixture","object":"response","status":status,"output":if text.is_empty(){json!([])}else{json!([{"id":"msg_host_fixture","type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}])}})
}
fn frame(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}
async fn upstream(State(state): State<PeerState>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    assert_eq!(
        parts.headers.get("authorization").unwrap(),
        "Bearer PRIVATE-EXEC-CREDENTIAL"
    );
    let bytes = axum::body::to_bytes(body, 512 * 1024).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    state.seen.lock().unwrap().push(Captured {
        method: parts.method.to_string(),
        path: parts.uri.to_string(),
        body: value,
    });
    if parts.uri.path().ends_with("/cancel") {
        return (
            [("content-type", "application/json")],
            response("cancelled", "").to_string(),
        )
            .into_response();
    }
    if state.mode == "json" {
        return (
            [("content-type", "application/json")],
            response(
                if parts.method == Method::POST {
                    "queued"
                } else {
                    "completed"
                },
                if parts.method == Method::POST {
                    ""
                } else {
                    "JSON host result"
                },
            )
            .to_string(),
        )
            .into_response();
    }
    let created = frame(
        json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_host_fixture","status":"in_progress"}}),
    );
    if state.mode == "large" {
        let text = "x".repeat(96 * 1024);
        let mut frames = Vec::new();
        if parts.method == Method::POST {
            frames.push(created);
            for (index, chunk) in text.as_bytes().chunks(256).enumerate() {
                frames.push(frame(json!({"type":"response.output_text.delta","sequence_number":index+1,"item_id":"msg_host_fixture","output_index":0,"content_index":0,"delta":std::str::from_utf8(chunk).unwrap()})));
            }
        }
        frames.push(frame(json!({"type":"response.completed","sequence_number":385,"response":response("completed",&text)})));
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(stream::iter(
                frames.into_iter().map(Ok::<_, std::io::Error>),
            )),
        )
            .into_response();
    }
    if state.mode == "drop" {
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(stream::iter([Err::<Bytes, _>(std::io::Error::other(
                "fixture disconnect after effect",
            ))])),
        )
            .into_response();
    }
    if state.mode == "hold" {
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(
                stream::once(async { Ok::<_, std::io::Error>(created) }).chain(stream::pending()),
            ),
        )
            .into_response();
    }
    if state.mode == "restart" && parts.method == Method::POST {
        let frames = vec![
            created,
            frame(
                json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"msg_host_fixture","output_index":0,"content_index":0,"delta":"head"}),
            ),
        ];
        return (
            [("content-type", "text/event-stream")],
            Body::from_stream(
                stream::iter(frames.into_iter().map(Ok::<_, std::io::Error>))
                    .chain(stream::pending()),
            ),
        )
            .into_response();
    }
    let (sequence, text) = if parts.method == Method::GET {
        (2, " tail")
    } else {
        (1, "head")
    };
    let mut frames = if parts.method == Method::GET {
        vec![frame(json!({"type":"keepalive","sequence_number":999}))]
    } else {
        vec![created]
    };
    frames.push(frame(json!({"type":"response.output_text.delta","sequence_number":sequence,"item_id":"msg_host_fixture","output_index":0,"content_index":0,"delta":text})));
    if parts.method == Method::GET {
        frames.push(frame(json!({"type":"response.completed","sequence_number":3,"response":response("completed","head tail")})));
    }
    (
        [("content-type", "text/event-stream")],
        Body::from_stream(stream::iter(
            frames.into_iter().map(Ok::<_, std::io::Error>),
        )),
    )
        .into_response()
}

struct MainServer(std::process::Child);
impl Drop for MainServer {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}
fn main_server(database: &mongodb::Database, root: &std::path::Path) -> (MainServer, String) {
    assert!(database.name().starts_with("tri_admin_test_"));
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let environment = dotenvy::from_path_iter(project.join(".env"))
        .unwrap()
        .map(Result::unwrap)
        .collect::<std::collections::HashMap<_, _>>();
    let log = std::fs::File::create(root.join(format!("ai-service-{}.log", uuid::Uuid::new_v4())))
        .unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_admin-api"));
    command
        .current_dir(project)
        .arg("serve")
        .env(
            "TRI_MONGODB_URI",
            environment.get("TRI_MONGODB_URI").unwrap(),
        )
        .env("TRI_MONGODB_DATABASE", database.name())
        .env("TRI_ADMIN_BIND", address.to_string())
        .env("TRI_JWT_SIGNING_KEY", support::KEY)
        .env(
            "TRI_OPEN_JWT_SIGNING_KEY",
            "fixture-independent-open-signing-key-at-least-32-bytes",
        )
        .env("TRI_SETTINGS_KEYS", format!("fixture:{}", "11".repeat(32)))
        .env("TRI_SETTINGS_ACTIVE_KEY", "fixture")
        .env("TRI_STORAGE_ROOT", root)
        .stdout(std::process::Stdio::from(log.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    (
        MainServer(command.spawn().unwrap()),
        format!("http://{address}"),
    )
}
async fn ready(server: &mut MainServer, base: &str) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(matches!(server.0.try_wait(), Ok(None)));
            if client
                .get(format!("{base}/health/ready"))
                .send()
                .await
                .is_ok_and(|value| value.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn real_main_restart_resumes_the_committed_cursor_without_repeating_generation() {
    use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
    let (app, _state, token, provider, peer) = fixture("restart").await;
    let root = tempfile::tempdir().unwrap();
    let body = input(&provider, true);
    let row = submitted(&app, &token, body.clone()).await;
    let id = row["id"].as_str().unwrap();
    let (mut first, base) = main_server(&app.database, root.path());
    ready(&mut first, &base).await;
    wait(&app, &token, id, "has-response").await;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let content = app
                .send(
                    "GET",
                    &format!("/api/v1/ai/runs/{id}/content"),
                    Value::Null,
                    Some(&token),
                )
                .await;
            if content.1["data"]["text"] == "head" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(first.0.try_wait(), Ok(None)));
    first.0.kill().unwrap();
    assert!(!first.0.wait().unwrap().success());
    drop(first);
    app.database.collection::<Document>("ai_runs").update_one(doc!{"_id":ObjectId::parse_str(id).unwrap()},doc!{"$set":{"leaseExpiresAt":DateTime::from_millis(0),"nextRunAt":DateTime::from_millis(0)}}).await.unwrap();
    let (mut second, base) = main_server(&app.database, root.path());
    ready(&mut second, &base).await;
    wait(&app, &token, id, "succeeded").await;
    assert_eq!(submitted(&app, &token, body).await["id"], id);
    let content = app
        .send(
            "GET",
            &format!("/api/v1/ai/runs/{id}/content"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(content.1["data"]["text"], "head tail");
    drop(second);
    {
        let seen = peer.state.seen.lock().unwrap();
        assert_eq!(
            seen.iter().filter(|value| value.method == "POST").count(),
            1
        );
        assert!(
            seen.iter().any(|value| value.path
                == "/v1/responses/resp_host_fixture?stream=true&starting_after=1")
        );
    }
    app.cleanup().await;
}
#[tokio::test]
async fn forged_execution_limits_cannot_decrypt_private_inputs_or_reach_the_upstream() {
    use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
    let (app, state, token, provider, peer) = fixture("json").await;
    let row = submitted(&app, &token, input(&provider, false)).await;
    let id = row["id"].as_str().unwrap();
    app.database.collection::<Document>("ai_runs").update_one(doc!{"_id":ObjectId::parse_str(id).unwrap()},doc!{"$set":{"executionDeadline":DateTime::from_millis(DateTime::now().timestamp_millis()+30*60*1000)}}).await.unwrap();
    let service = worker::start_service(state.clone());
    let failed = wait(&app, &token, id, "failed").await;
    assert_eq!(failed["mayHaveBeenSubmitted"], false);
    assert_eq!(failed["errorCode"], "SECRET_STORE_UNAVAILABLE");
    worker::stop_service(&state);
    service.await.unwrap();
    assert!(peer.state.seen.lock().unwrap().is_empty());
    app.cleanup().await;
}

#[tokio::test]
async fn a_long_streamed_result_finishes_with_bounded_progress_work_and_no_second_generation() {
    use tower::ServiceExt;
    let (app, state, token, provider, peer) = fixture("large").await;
    let service = worker::start_service(state.clone());
    let row = submitted(&app, &token, input(&provider, true)).await;
    let id = row["id"].as_str().unwrap();
    wait(&app, &token, id, "succeeded").await;
    let reply = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/ai/runs/{id}/content"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(reply.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let content: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(content["data"]["text"].as_str().unwrap().len(), 96 * 1024);
    worker::stop_service(&state);
    service.await.unwrap();
    assert_eq!(
        peer.state
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.method == "POST")
            .count(),
        1
    );
    app.cleanup().await;
}
impl Peer {
    async fn start(mode: &'static str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let state = PeerState {
            mode,
            seen: Default::default(),
        };
        let router = Router::new().fallback(upstream).with_state(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { url, state, task }
    }
}
async fn fixture(mode: &'static str) -> (TestApp, AppState, String, Value, Peer) {
    let peer = Peer::start(mode).await;
    let mut app = TestApp::new().await;
    let config = Config::from_lookup(|name| match name {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        _ => None,
    })
    .unwrap();
    let state = AppState::new(config, Some(app.database.clone()));
    app.router = build_router(state.clone());
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap().to_owned();
    let (status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"AI execution","code":"ai.execution","enabled":true,"configuration":{"kind":"http","baseUrl":peer.url},"secret":{"kind":"http","headerName":"authorization","value":"Bearer PRIVATE-EXEC-CREDENTIAL"}}),Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let (status,provider)=app.send("POST","/api/v1/ai/providers",json!({"name":"Executed provider","profileId":profile["data"]["id"],"profileVersion":1,"enabled":true,"models":[{"id":"fixture-model","label":"Fixture","maxOutputTokens":512}]}),Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    (app, state, token, provider["data"].clone(), peer)
}
fn input(provider: &Value, stream: bool) -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"providerId":provider["id"],"providerVersion":1,"model":"fixture-model","input":"PRIVATE-EXEC-INPUT","instructions":null,"maxOutputTokens":128,"stream":stream})
}
async fn submitted(app: &TestApp, token: &str, body: Value) -> Value {
    let (status, row) = app.send("POST", "/api/v1/ai/runs", body, Some(token)).await;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED, "{row}");
    row["data"].clone()
}
async fn wait(app: &TestApp, token: &str, id: &str, wanted: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let (status, row) = app
            .send(
                "GET",
                &format!("/api/v1/ai/runs/{id}"),
                Value::Null,
                Some(token),
            )
            .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let row = row["data"].clone();
        if wanted == "has-response" && row["hasResponse"] == true || row["state"] == wanted {
            return row;
        }
        assert!(Instant::now() < deadline, "{row}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
#[tokio::test]
async fn background_json_and_interrupted_streams_finish_with_one_generation_and_get_only_recovery()
{
    for mode in ["json", "resume"] {
        let (app, state, token, provider, peer) = fixture(mode).await;
        let service = worker::start_service(state.clone());
        let body = input(&provider, mode != "json");
        let row = submitted(&app, &token, body.clone()).await;
        let id = row["id"].as_str().unwrap();
        wait(&app, &token, id, "succeeded").await;
        let (status, content) = app
            .send(
                "GET",
                &format!("/api/v1/ai/runs/{id}/content"),
                Value::Null,
                Some(&token),
            )
            .await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(
            content["data"]["text"],
            if mode == "json" {
                "JSON host result"
            } else {
                "head tail"
            }
        );
        assert_eq!(content["data"]["responseStatus"], "completed");
        assert_eq!(submitted(&app, &token, body).await["id"], id);
        worker::stop_service(&state);
        service.await.unwrap();
        {
            let seen = peer.state.seen.lock().unwrap();
            assert_eq!(
                seen.iter()
                    .filter(|row| row.method == "POST" && row.path == "/v1/responses")
                    .count(),
                1
            );
            assert!(seen.iter().any(|row| row.method == "GET"));
            if mode == "resume" {
                assert!(seen.iter().any(|row| row.path
                    == "/v1/responses/resp_host_fixture?stream=true&starting_after=1"));
            }
            assert_eq!(seen[0].body["input"], "PRIVATE-EXEC-INPUT");
        }
        app.cleanup().await;
    }
}
#[tokio::test]
async fn lost_response_identity_stays_unknown_without_a_second_generation() {
    let (app, state, token, provider, peer) = fixture("drop").await;
    let service = worker::start_service(state.clone());
    let row = submitted(&app, &token, input(&provider, true)).await;
    let id = row["id"].as_str().unwrap();
    let unknown = wait(&app, &token, id, "unknown").await;
    assert_eq!(unknown["hasResponse"], false);
    assert_eq!(unknown["mayHaveBeenSubmitted"], true);
    tokio::time::sleep(Duration::from_millis(800)).await;
    worker::stop_service(&state);
    service.await.unwrap();
    {
        let seen = peer.state.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "POST");
    }
    app.cleanup().await;
}
#[tokio::test]
async fn cancellation_is_recorded_before_one_upstream_cancel_and_requires_confirmation() {
    let (app, state, token, provider, peer) = fixture("hold").await;
    let service = worker::start_service(state.clone());
    let row = submitted(&app, &token, input(&provider, true)).await;
    let id = row["id"].as_str().unwrap();
    let running = wait(&app, &token, id, "has-response").await;
    let (status, cancel) = app
        .send(
            "POST",
            &format!("/api/v1/ai/runs/{id}/cancel"),
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":running["version"]}),
            Some(&token),
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED, "{cancel}");
    let confirmed = wait(&app, &token, id, "cancelled").await;
    assert_eq!(confirmed["cancelConfirmed"], true);
    worker::stop_service(&state);
    service.await.unwrap();
    {
        let seen = peer.state.seen.lock().unwrap();
        assert_eq!(
            seen.iter()
                .filter(|row| row.path.ends_with("/cancel"))
                .count(),
            1
        );
        assert_eq!(
            seen.iter()
                .filter(|row| row.path == "/v1/responses")
                .count(),
            1
        );
    }
    app.cleanup().await;
}
#[tokio::test]
async fn an_expired_queued_intent_is_closed_without_sending_a_generation() {
    use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
    let (app, state, token, provider, peer) = fixture("json").await;
    let row = submitted(&app, &token, input(&provider, false)).await;
    let id = ObjectId::parse_str(row["id"].as_str().unwrap()).unwrap();
    app.database
        .collection::<Document>("ai_runs")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"executionDeadline":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let service = worker::start_service(state.clone());
    let deadline = Instant::now() + Duration::from_secs(4);
    let observed = loop {
        let row = app
            .database
            .collection::<Document>("ai_runs")
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap();
        if row.get_str("state").unwrap() != "pending" || Instant::now() >= deadline {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    worker::stop_service(&state);
    service.await.unwrap();
    let effects = peer.state.seen.lock().unwrap().len();
    app.cleanup().await;
    assert_eq!(observed.get_str("state").unwrap(), "failed");
    assert_eq!(effects, 0);
}
#[tokio::test]
async fn current_permissions_and_profile_versions_stop_inflight_owned_streams() {
    use mongodb::bson::{Document, doc, oid::ObjectId};
    for change in ["permissions", "profile", "credentials"] {
        let (app, state, token, provider, peer) = fixture("hold").await;
        let service = worker::start_service(state.clone());
        let row = submitted(&app, &token, input(&provider, true)).await;
        let id = row["id"].as_str().unwrap();
        wait(&app, &token, id, "has-response").await;
        if change == "permissions" {
            app.database
                .collection::<Document>("roles")
                .update_one(doc! {"protected":true}, doc! {"$set":{"permissions":[]}})
                .await
                .unwrap();
        } else {
            assert_eq!(
                app.send(
                    "PATCH",
                    &format!(
                        "/api/v1/settings/profiles/{}",
                        provider["profileId"].as_str().unwrap()
                    ),
                    if change=="credentials"{json!({"version":1,"secret":{"kind":"http","headerName":"authorization","value":"Bearer ROTATED-EXEC-FIXTURE"}})}else{json!({"version":1,"description":"changed while running"})},
                    Some(&token)
                )
                .await
                .0,
                axum::http::StatusCode::OK
            );
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let row = app
                .database
                .collection::<Document>("ai_runs")
                .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
                .await
                .unwrap()
                .unwrap();
            if row.get_str("state").unwrap() == "unknown" {
                assert_eq!(
                    row.get_str("errorCode").unwrap(),
                    if change == "permissions" {
                        "PERMISSION_REVOKED"
                    } else {
                        "BINDING_CHANGED"
                    }
                );
                break;
            }
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        if change == "permissions" {
            assert_eq!(
                app.send(
                    "GET",
                    &format!("/api/v1/ai/runs/{id}/content"),
                    Value::Null,
                    Some(&token)
                )
                .await
                .0,
                axum::http::StatusCode::FORBIDDEN
            );
        }
        worker::stop_service(&state);
        service.await.unwrap();
        assert_eq!(peer.state.seen.lock().unwrap().len(), 1);
        app.cleanup().await;
    }
}
#[tokio::test]
async fn a_confirmed_completion_commit_with_a_lost_reply_never_repeats_generation() {
    use mongodb::bson::oid::ObjectId;
    let (app, state, token, provider, peer) = fixture("json").await;
    let row = submitted(&app, &token, input(&provider, false)).await;
    let id = row["id"].as_str().unwrap();
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let environment = dotenvy::from_path_iter(project.join(".env"))
        .unwrap()
        .map(Result::unwrap)
        .collect::<std::collections::HashMap<_, _>>();
    let proxy = mongo_faults::MongoCommitFault::start_for(
        environment.get("TRI_MONGODB_URI").unwrap(),
        app.database.name(),
        ObjectId::parse_str(id).unwrap(),
        "ai_runs",
        "succeeded",
    )
    .await;
    let mut proxied = state.clone();
    proxied.database = Some(
        mongodb::Client::with_uri_str(&proxy.uri)
            .await
            .unwrap()
            .database(app.database.name()),
    );
    let service = worker::start_service(proxied.clone());
    proxy.wait_for_lost_commit().await;
    wait(&app, &token, id, "succeeded").await;
    worker::stop_service(&proxied);
    service.await.unwrap();
    assert_eq!(proxy.dropped(), 1);
    assert!(proxy.commits() >= 2);
    assert_eq!(proxy.repeated_body(), 0);
    assert_eq!(
        peer.state
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|value| value.method == "POST")
            .count(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn independent_service_instances_share_two_execution_leases() {
    use mongodb::bson::{DateTime, Document, doc};
    let (app, state, token, provider, peer) = fixture("hold").await;
    for _ in 0..3 {
        submitted(&app, &token, input(&provider, true)).await;
    }
    let other = AppState::new(state.config.as_ref().clone(), Some(app.database.clone()));
    let first = worker::start_service(state.clone());
    let second = worker::start_service(other.clone());
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if peer.state.seen.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(peer.state.seen.lock().unwrap().len(), 2);
    assert_eq!(
        app.database
            .collection::<Document>("ai_runs")
            .count_documents(doc! {"leaseExpiresAt":{"$gt":DateTime::now()}})
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        app.database
            .collection::<Document>("ai_runs")
            .count_documents(doc! {"state":"pending"})
            .await
            .unwrap(),
        1
    );
    worker::stop_service(&state);
    worker::stop_service(&other);
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(peer.state.seen.lock().unwrap().len(), 2);
    app.cleanup().await;
}
#[tokio::test]
async fn an_unsupported_database_model_cannot_start_a_paid_generation() {
    use mongodb::bson::{Document, doc};
    let (app, state, token, provider, peer) = fixture("json").await;
    submitted(&app, &token, input(&provider, false)).await;
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"ai"}, doc! {"$set":{"version":99}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let service = worker::start_service(state.clone());
    tokio::time::sleep(Duration::from_secs(2)).await;
    worker::stop_service(&state);
    service.await.unwrap();
    let effects = peer.state.seen.lock().unwrap().len();
    app.cleanup().await;
    assert_eq!(effects, 0);
}
