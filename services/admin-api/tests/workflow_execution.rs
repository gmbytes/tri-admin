mod support;
use admin_api::{
    build_router,
    config::Config,
    modules::{ai, workflows::worker},
    state::AppState,
};
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use support::TestApp;
#[derive(Clone)]
struct Upstream {
    bad: bool,
    requests: Arc<Mutex<Vec<Value>>>,
    results: Arc<Mutex<HashMap<String, String>>>,
    hold: Arc<AtomicBool>,
    cancelled: Arc<Mutex<HashSet<String>>>,
    cancel_count: Arc<AtomicUsize>,
}
struct Peer {
    url: String,
    state: Upstream,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn response(id: &str, status: &str, text: &str) -> Value {
    json!({"id":id,"object":"response","status":status,"output":if text.is_empty(){json!([])}else{json!([{"id":format!("msg_{id}"),"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}])}})
}
async fn upstream(State(state): State<Upstream>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, 512 * 1024).await.unwrap();
    if parts.uri.path().ends_with("/cancel") {
        let id = parts
            .uri
            .path()
            .trim_end_matches("/cancel")
            .rsplit('/')
            .next()
            .unwrap();
        state.cancelled.lock().unwrap().insert(id.into());
        state.cancel_count.fetch_add(1, Ordering::SeqCst);
        return (
            [("content-type", "application/json")],
            response(id, "cancelled", "").to_string(),
        )
            .into_response();
    }
    if parts.method == Method::POST {
        let value: Value = serde_json::from_slice(&body).unwrap();
        let mut requests = state.requests.lock().unwrap();
        let index = requests.len() + 1;
        let analysis = value["text"]["format"]["name"] == "tri_admin_requirements_v1";
        let text = if analysis && !state.bad {
            json!({"summary":"Structured analysis","modules":[{"key":"worlds","label":"Worlds","description":"Game worlds","fields":[]}],"risks":[],"questions":[]}).to_string()
        } else {
            format!("RESULT-STEP-{index}")
        };
        let id = format!("resp_workflow_{index}");
        requests.push(value);
        drop(requests);
        state.results.lock().unwrap().insert(id.clone(), text);
        return (
            [("content-type", "application/json")],
            response(&id, "queued", "").to_string(),
        )
            .into_response();
    }
    let id = parts.uri.path().rsplit('/').next().unwrap();
    let text = state.results.lock().unwrap().get(id).cloned().unwrap();
    let cancelled = state.cancelled.lock().unwrap().contains(id);
    let pending = state.hold.load(Ordering::SeqCst) && !cancelled;
    (
        [("content-type", "application/json")],
        response(
            id,
            if cancelled {
                "cancelled"
            } else if pending {
                "in_progress"
            } else {
                "completed"
            },
            if cancelled || pending { "" } else { &text },
        )
        .to_string(),
    )
        .into_response()
}
async fn fixture(bad: bool) -> (TestApp, AppState, String, Value, Peer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let peer_state = Upstream {
        bad,
        requests: Default::default(),
        results: Default::default(),
        hold: Default::default(),
        cancelled: Default::default(),
        cancel_count: Default::default(),
    };
    let router = Router::new()
        .fallback(upstream)
        .with_state(peer_state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let peer = Peer {
        url,
        state: peer_state,
        task,
    };
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
    let (status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"Workflow upstream","code":"workflow.upstream","enabled":true,"configuration":{"kind":"http","baseUrl":peer.url}}),Some(&token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status,provider)=app.send("POST","/api/v1/ai/providers",json!({"name":"Workflow provider","profileId":profile["data"]["id"],"profileVersion":1,"enabled":true,"models":[{"id":"fixture-model","label":"Fixture","maxOutputTokens":1024}]}),Some(&token)).await;
    assert_eq!(status, StatusCode::CREATED);
    (app, state, token, provider["data"].clone(), peer)
}
fn plan() -> Value {
    json!({"mode":"project","steps":[{"id":"analysis","title":"Analysis","prompt":"PRIVATE-ORIGINAL-PLAN","dependsOn":[],"expected":"analysis"},{"id":"design","title":"Design","prompt":"Design from the analysis","dependsOn":["analysis"],"expected":"text"},{"id":"deliver","title":"Deliver","prompt":"Deliver implementation","dependsOn":["design"],"expected":"text"}]})
}
async fn saved(app: &TestApp, token: &str) -> Value {
    let (status,value)=app.send("POST","/api/v1/workflows/plans",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Executed plan","plan":plan()}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
fn inputs() -> Value {
    json!({"requirements":"PRIVATE-WORKFLOW-REQUIREMENTS","scene":"Game server operations","constraints":"Rust MongoDB Vue"})
}
fn start(provider: &Value, plan: &Value) -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"planId":plan["id"],"planVersion":1,"providerId":provider["id"],"providerVersion":1,"model":"fixture-model","maxOutputTokens":512,"stream":false,"inputs":inputs()})
}
async fn submitted(app: &TestApp, token: &str, path: &str, body: Value) -> Value {
    let (status, value) = app.send("POST", path, body, Some(token)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{value}");
    value["data"].clone()
}
async fn wait(app: &TestApp, token: &str, id: &str, wanted: &str) -> Value {
    wait_for(app, token, id, wanted, Duration::from_secs(20)).await
}
async fn wait_for(app: &TestApp, token: &str, id: &str, wanted: &str, duration: Duration) -> Value {
    let started = Instant::now();
    let deadline = started + duration;
    let mut observed = false;
    loop {
        let (status, value) = app
            .send(
                "GET",
                &format!("/api/v1/workflows/runs/{id}"),
                Value::Null,
                Some(token),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let row = value["data"].clone();
        if row["state"] == wanted {
            return row;
        }
        if !observed && started.elapsed() >= Duration::from_secs(20) {
            observed = true;
            let status = app
                .database
                .client()
                .database("admin")
                .run_command(mongodb::bson::doc! {"serverStatus":1,"transactions":1})
                .await
                .ok();
            let transactions=status.as_ref().and_then(|value|value.get_document("transactions").ok()).map(|value|serde_json::json!({"currentOpen":value.get_i64("currentOpen").ok(),"currentActive":value.get_i64("currentActive").ok(),"currentInactive":value.get_i64("currentInactive").ok()}));
            let diagnostic = json!({"elapsedSeconds":started.elapsed().as_secs(),"parent":row,"transactions":transactions});
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.superpowers/sdd/2026-10-24-tri-admin-greenfield")
                .join(format!("workflow-recovery-observation-{id}.json"));
            std::fs::write(path, serde_json::to_vec_pretty(&diagnostic).unwrap()).unwrap();
        }
        assert!(Instant::now() < deadline, "{row}");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}
#[tokio::test]
async fn actual_workflow_uses_fixed_revisions_dependency_outputs_and_original_child_uuids() {
    let (app, state, token, provider, peer) = fixture(false).await;
    let definition = saved(&app, &token).await;
    let body = start(&provider, &definition);
    let row = submitted(&app, &token, "/api/v1/workflows/runs", body.clone()).await;
    let id = row["id"].as_str().unwrap();
    let mut changed = plan();
    changed["steps"][0]["prompt"] = json!("ALTERED-AFTER-SUBMISSION");
    assert_eq!(
        app.send(
            "PATCH",
            &format!(
                "/api/v1/workflows/plans/{}",
                definition["id"].as_str().unwrap()
            ),
            json!({"version":1,"name":"Changed plan","plan":changed}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let ai = ai::worker::start_service(state.clone());
    let workflow = worker::start_service(state.clone());
    wait(&app, &token, id, "succeeded").await;
    assert_eq!(
        submitted(&app, &token, "/api/v1/workflows/runs", body).await["id"],
        id
    );
    let content = app
        .send(
            "GET",
            &format!("/api/v1/workflows/runs/{id}/content"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(content.0, StatusCode::OK);
    assert_eq!(content.1["data"]["steps"].as_array().unwrap().len(), 3);
    assert_eq!(content.1["data"]["steps"][2]["text"], "RESULT-STEP-3");
    worker::stop_service(&state);
    ai::worker::stop_service(&state);
    workflow.await.unwrap();
    ai.await.unwrap();
    {
        let requests = peer.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests[0]["input"]
                .as_str()
                .unwrap()
                .contains("PRIVATE-ORIGINAL-PLAN")
        );
        assert!(
            !requests[0]["input"]
                .as_str()
                .unwrap()
                .contains("ALTERED-AFTER-SUBMISSION")
        );
        assert!(
            requests[1]["input"]
                .as_str()
                .unwrap()
                .contains("Structured analysis")
        );
        assert!(
            requests[2]["input"]
                .as_str()
                .unwrap()
                .contains("RESULT-STEP-2")
        );
    }
    app.cleanup().await;
}
#[tokio::test]
async fn malformed_analysis_fails_the_workflow_without_submitting_later_steps() {
    let (app, state, token, provider, peer) = fixture(true).await;
    let plan = saved(&app, &token).await;
    let row = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    let ai = ai::worker::start_service(state.clone());
    let workflow = worker::start_service(state.clone());
    let failed = wait(&app, &token, row["id"].as_str().unwrap(), "failed").await;
    assert_eq!(failed["completedSteps"], 0);
    worker::stop_service(&state);
    ai::worker::stop_service(&state);
    workflow.await.unwrap();
    ai.await.unwrap();
    assert_eq!(peer.state.requests.lock().unwrap().len(), 1);
    app.cleanup().await;
}
#[tokio::test]
async fn standalone_analysis_is_a_durable_typed_workflow() {
    let (app, state, token, provider, peer) = fixture(false).await;
    let body = json!({"requestId":uuid::Uuid::new_v4().to_string(),"providerId":provider["id"],"providerVersion":1,"model":"fixture-model","maxOutputTokens":512,"stream":false,"inputs":inputs()});
    let row = submitted(&app, &token, "/api/v1/workflows/analyses", body).await;
    let ai = ai::worker::start_service(state.clone());
    let workflow = worker::start_service(state.clone());
    let id = row["id"].as_str().unwrap();
    wait(&app, &token, id, "succeeded").await;
    let content = app
        .send(
            "GET",
            &format!("/api/v1/workflows/runs/{id}/content"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(
        content.1["data"]["steps"][0]["analysis"]["modules"][0]["key"],
        "worlds"
    );
    worker::stop_service(&state);
    ai::worker::stop_service(&state);
    workflow.await.unwrap();
    ai.await.unwrap();
    assert_eq!(peer.state.requests.lock().unwrap().len(), 1);
    app.cleanup().await;
}
#[tokio::test]
async fn queued_cancellation_is_durable_and_never_admits_a_child_generation() {
    let (app, state, token, provider, peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    let body = start(&provider, &plan);
    let row = submitted(&app, &token, "/api/v1/workflows/runs", body.clone()).await;
    let id = row["id"].as_str().unwrap();
    let cancelled = submitted(
        &app,
        &token,
        &format!("/api/v1/workflows/runs/{id}/cancel"),
        json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1}),
    )
    .await;
    assert_eq!(cancelled["state"], "cancelled");
    let ai = ai::worker::start_service(state.clone());
    let workflow = worker::start_service(state.clone());
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        submitted(&app, &token, "/api/v1/workflows/runs", body).await["id"],
        id
    );
    worker::stop_service(&state);
    ai::worker::stop_service(&state);
    workflow.await.unwrap();
    ai.await.unwrap();
    assert!(peer.state.requests.lock().unwrap().is_empty());
    app.cleanup().await;
}
#[tokio::test]
async fn active_cancellation_propagates_once_and_stops_all_later_steps() {
    let (app, state, token, provider, peer) = fixture(false).await;
    peer.state.hold.store(true, Ordering::SeqCst);
    let plan = saved(&app, &token).await;
    let row = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    let id = row["id"].as_str().unwrap();
    let ai = ai::worker::start_service(state.clone());
    let workflow = worker::start_service(state.clone());
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let parent = app
                .send(
                    "GET",
                    &format!("/api/v1/workflows/runs/{id}"),
                    Value::Null,
                    Some(&token),
                )
                .await
                .1["data"]
                .clone();
            if let Some(child) = parent["nodes"][0]["aiRunId"].as_str() {
                let info = app
                    .send(
                        "GET",
                        &format!("/api/v1/ai/runs/{child}"),
                        Value::Null,
                        Some(&token),
                    )
                    .await;
                if info.1["data"]["hasResponse"] == true {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let cancel_id = uuid::Uuid::new_v4().to_string();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let current = app
            .send(
                "GET",
                &format!("/api/v1/workflows/runs/{id}"),
                Value::Null,
                Some(&token),
            )
            .await
            .1["data"]
            .clone();
        let result = app
            .send(
                "POST",
                &format!("/api/v1/workflows/runs/{id}/cancel"),
                json!({"requestId":cancel_id,"version":current["version"]}),
                Some(&token),
            )
            .await;
        if result.0 == StatusCode::ACCEPTED {
            break;
        }
        assert_eq!(result.0, StatusCode::CONFLICT);
        assert!(Instant::now() < deadline);
    }
    let stopped =
        tokio::time::timeout(Duration::from_secs(12), wait(&app, &token, id, "cancelled")).await;
    worker::stop_service(&state);
    ai::worker::stop_service(&state);
    workflow.await.unwrap();
    ai.await.unwrap();
    let generations = peer.state.requests.lock().unwrap().len();
    let cancels = peer.state.cancel_count.load(Ordering::SeqCst);
    app.cleanup().await;
    let stopped = stopped.expect("active workflow cancellation must be confirmed");
    assert_eq!(stopped["cancelConfirmed"], true);
    assert_eq!(generations, 1);
    assert_eq!(cancels, 1);
    assert!(
        stopped["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["state"] != "pending")
    );
}
struct Main(std::process::Child, std::path::PathBuf);
impl Drop for Main {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
        if std::thread::panicking() {
            let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.superpowers/sdd/2026-10-24-tri-admin-greenfield")
                .join(format!("workflow-process-failure-{}.log", self.0.id()));
            let _ = std::fs::copy(&self.1, target);
        }
    }
}
fn main_server(database: &mongodb::Database, root: &std::path::Path) -> (Main, String) {
    assert!(database.name().starts_with("tri_admin_test_"));
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = port.local_addr().unwrap();
    drop(port);
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let environment = dotenvy::from_path_iter(project.join(".env"))
        .unwrap()
        .map(Result::unwrap)
        .collect::<std::collections::HashMap<_, _>>();
    let log_path = root.join(format!("workflow-main-{}.log", uuid::Uuid::new_v4()));
    let log = std::fs::File::create(&log_path).unwrap();
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
        Main(command.spawn().unwrap(), log_path),
        format!("http://{address}"),
    )
}
async fn ready(main: &mut Main, url: &str) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(matches!(main.0.try_wait(), Ok(None)));
            if client
                .get(format!("{url}/health/ready"))
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
async fn first_child(app: &TestApp, token: &str, id: &str, known_response: bool) -> String {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let row = app
                .send(
                    "GET",
                    &format!("/api/v1/workflows/runs/{id}"),
                    Value::Null,
                    Some(token),
                )
                .await
                .1["data"]
                .clone();
            if let Some(child) = row["nodes"][0]["aiRunId"].as_str() {
                if !known_response {
                    return child.to_owned();
                }
                let child_row = app
                    .send(
                        "GET",
                        &format!("/api/v1/ai/runs/{child}"),
                        Value::Null,
                        Some(token),
                    )
                    .await;
                if child_row.1["data"]["hasResponse"] == true {
                    return child.to_owned();
                }
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn a_real_process_restart_recovers_the_same_parent_and_child_generations() {
    use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
    let (app, _state, token, provider, peer) = fixture(false).await;
    peer.state.hold.store(true, Ordering::SeqCst);
    let plan = saved(&app, &token).await;
    let body = start(&provider, &plan);
    let row = submitted(&app, &token, "/api/v1/workflows/runs", body.clone()).await;
    let id = row["id"].as_str().unwrap();
    let root = tempfile::tempdir().unwrap();
    let (mut first, url) = main_server(&app.database, root.path());
    ready(&mut first, &url).await;
    let child = first_child(&app, &token, id, true).await;
    first.0.kill().unwrap();
    assert!(!first.0.wait().unwrap().success());
    drop(first);
    for (collection, id) in [("workflow_runs", id), ("ai_runs", child.as_str())] {
        app.database.collection::<Document>(collection).update_one(doc!{"_id":ObjectId::parse_str(id).unwrap()},doc!{"$set":{"leaseExpiresAt":DateTime::from_millis(0),"nextRunAt":DateTime::from_millis(0)}}).await.unwrap();
    }
    peer.state.hold.store(false, Ordering::SeqCst);
    let (mut second, url) = main_server(&app.database, root.path());
    ready(&mut second, &url).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let served = client
        .get(format!("{url}/api/v1/workflows/runs/{id}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(served.status(), StatusCode::OK);
    let served: Value = serde_json::from_slice(&served.bytes().await.unwrap()).unwrap();
    assert_eq!(served["data"]["id"], id);
    let parameter = app
        .database
        .client()
        .database("admin")
        .run_command(doc! {"getParameter":1,"transactionLifetimeLimitSeconds":1})
        .await
        .unwrap();
    let lifetime = parameter
        .get_i64("transactionLifetimeLimitSeconds")
        .or_else(|_| {
            parameter
                .get_i32("transactionLifetimeLimitSeconds")
                .map(i64::from)
        })
        .unwrap();
    assert!((1..=120).contains(&lifetime));
    // A killed process can leave an in-flight transaction until MongoDB's
    // configured lifetime plus periodic cleanup. Observe this same new process.
    let recovery = Duration::from_secs((lifetime + (lifetime / 2).clamp(1, 60) + 30) as u64);
    wait_for(&app, &token, id, "succeeded", recovery).await;
    assert_eq!(
        submitted(&app, &token, "/api/v1/workflows/runs", body).await["id"],
        id
    );
    drop(second);
    assert_eq!(peer.state.requests.lock().unwrap().len(), 3);
    assert_eq!(
        app.database
            .collection::<Document>("ai_runs")
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
    app.cleanup().await;
}
#[tokio::test]
async fn accepted_children_recheck_workflow_authority_at_the_generation_boundary() {
    use mongodb::bson::{Document, doc};
    let (app, state, token, provider, peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    let row = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    let workflow = worker::start_service(state.clone());
    let child = first_child(&app, &token, row["id"].as_str().unwrap(), false).await;
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"protected":true},
            doc! {"$set":{"permissions":["ai.read","ai.invoke"]}},
        )
        .await
        .unwrap();
    let ai = ai::worker::start_service(state.clone());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let value = app
            .send(
                "GET",
                &format!("/api/v1/ai/runs/{child}"),
                Value::Null,
                Some(&token),
            )
            .await
            .1["data"]
            .clone();
        if value["state"] == "failed" {
            assert_eq!(value["mayHaveBeenSubmitted"], false);
            break;
        }
        assert!(Instant::now() < deadline, "{value}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/ai/runs/{child}/content"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    worker::stop_service(&state);
    ai::worker::stop_service(&state);
    workflow.await.unwrap();
    ai.await.unwrap();
    assert!(peer.state.requests.lock().unwrap().is_empty());
    app.cleanup().await;
}
async fn download(
    app: &TestApp,
    token: &str,
    path: &str,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    use tower::ServiceExt;
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, headers, body.to_vec())
}
#[tokio::test]
async fn markdown_export_is_private_immutable_idempotent_and_expires_without_regeneration() {
    use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
    let (app, _state, token, provider, _peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    let run = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    let run_id = run["id"].as_str().unwrap();
    let body = json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1});
    let path = format!("/api/v1/workflows/runs/{run_id}/exports");
    let (status, value) = app.send("POST", &path, body.clone(), Some(&token)).await;
    if status != StatusCode::CREATED {
        app.cleanup().await;
        assert_eq!(status, StatusCode::CREATED, "{value}");
        return;
    }
    let export = value["data"].clone();
    let id = export["id"].as_str().unwrap();
    let uri = format!("/api/v1/workflows/exports/{id}/content");
    let (status, headers, bytes) = download(&app, &token, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(headers["content-type"], "text/markdown; charset=utf-8");
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("PRIVATE-ORIGINAL-PLAN"));
    assert!(text.contains("PRIVATE-WORKFLOW-REQUIREMENTS"));
    assert_eq!(export["size"], bytes.len());
    use sha2::{Digest, Sha256};
    assert_eq!(export["sha256"], format!("{:x}", Sha256::digest(&bytes)));
    assert!(
        export["fileName"]
            .as_str()
            .unwrap()
            .starts_with("workflow-")
    );
    let replay = app.send("POST", &path, body.clone(), Some(&token)).await;
    assert_eq!(replay.0, StatusCode::CREATED);
    assert_eq!(replay.1["data"]["id"], id);
    let invalid = json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"directory":"C:/not-an-export-target"});
    assert_eq!(
        app.send("POST", &path, invalid, Some(&token)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.database
        .collection::<Document>("workflow_exports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert_eq!(download(&app, &token, &uri).await.0, StatusCode::CONFLICT);
    assert_eq!(
        app.send("POST", &path, body, Some(&token)).await.1["data"]["id"],
        id
    );
    assert_eq!(
        app.database
            .collection::<Document>("workflow_exports")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn failed_export_audit_never_leaves_an_artifact_or_plaintext() {
    use mongodb::bson::{Document, doc};
    let (app, _state, token, provider, _peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    let run = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"workflows.exports.create"}},"validationLevel":"strict"}).await.unwrap();
    let result = app
        .send(
            "POST",
            &format!(
                "/api/v1/workflows/runs/{}/exports",
                run["id"].as_str().unwrap()
            ),
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1}),
            Some(&token),
        )
        .await;
    let mut counts = Vec::new();
    for collection in ["workflow_exports", "workflow_export_payloads"] {
        counts.push(
            app.database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
        );
    }
    app.cleanup().await;
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(counts, vec![0, 0]);
}
fn selection(provider: &Value, plan: &Value) -> Value {
    json!({"providerId":provider["id"],"providerVersion":1,"model":"fixture-model","maxOutputTokens":512,"stream":false,"mode":"project","planId":plan["id"],"planVersion":1})
}
async fn session(app: &TestApp, token: &str, provider: &Value, plan: &Value, name: &str) -> Value {
    let (status,value)=app.send("POST","/api/v1/workflows/sessions",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":name,"selection":selection(provider,plan),"draft":inputs()}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
#[tokio::test]
async fn encrypted_sessions_restore_form_choices_and_branch_fixed_run_history_without_model_calls()
{
    use mongodb::bson::{Document, doc};
    let (app, _state, token, provider, peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    let run1 = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    let run2 = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    let saved = session(&app, &token, &provider, &plan, "Private session").await;
    let id = saved["id"].as_str().unwrap();
    assert!(!saved.to_string().contains("PRIVATE-WORKFLOW"));
    let detail = app
        .send(
            "GET",
            &format!("/api/v1/workflows/sessions/{id}/content"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(detail.0, StatusCode::OK);
    assert_eq!(
        detail.1["data"]["draft"]["requirements"],
        "PRIVATE-WORKFLOW-REQUIREMENTS"
    );
    assert_eq!(detail.1["data"]["selection"]["model"], "fixture-model");
    let first_body = json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"parentMessageId":null,"runId":run1["id"]});
    let first = app
        .send(
            "POST",
            &format!("/api/v1/workflows/sessions/{id}/messages"),
            first_body.clone(),
            Some(&token),
        )
        .await;
    assert_eq!(first.0, StatusCode::CREATED);
    let first_id = first.1["data"]["id"].as_str().unwrap();
    let recovered = app
        .send(
            "GET",
            &format!(
                "/api/v1/workflows/messages/by-request/{}",
                first.1["data"]["requestId"].as_str().unwrap()
            ),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(recovered.0, StatusCode::OK);
    assert_eq!(recovered.1["data"]["id"], first_id);
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/workflows/sessions/{id}/messages"),
            first_body,
            Some(&token)
        )
        .await
        .1["data"]["id"],
        first_id
    );
    let second=app.send("POST",&format!("/api/v1/workflows/sessions/{id}/messages"),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":2,"parentMessageId":first_id,"runId":run2["id"]}),Some(&token)).await;
    assert_eq!(second.0, StatusCode::CREATED);
    let second_id = second.1["data"]["id"].as_str().unwrap();
    let branch=app.send("POST",&format!("/api/v1/workflows/sessions/{id}/messages"),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":3,"parentMessageId":first_id,"runId":run2["id"]}),Some(&token)).await;
    assert_eq!(branch.0, StatusCode::CREATED);
    let list = app
        .send(
            "GET",
            &format!("/api/v1/workflows/sessions/{id}/messages?pageSize=100"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(list.1["meta"]["total"], 3);
    assert_eq!(
        list.1["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["parentMessageId"] == first_id)
            .count(),
        2
    );
    let patch = json!({"version":4,"name":"Restored selection","selection":selection(&provider,&plan),"draft":inputs(),"currentMessageId":second_id,"currentNode":"analysis"});
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/workflows/sessions/{id}"),
            patch,
            Some(&token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let restored = app
        .send(
            "GET",
            &format!("/api/v1/workflows/sessions/{id}/content"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(restored.1["data"]["currentMessageId"], second_id);
    assert_eq!(restored.1["data"]["currentNode"], "analysis");
    let history = app
        .send(
            "GET",
            &format!("/api/v1/workflows/messages/{second_id}/content"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(history.0, StatusCode::OK);
    assert_eq!(
        history.1["data"]["run"]["inputs"]["requirements"],
        "PRIVATE-WORKFLOW-REQUIREMENTS"
    );
    let ciphertext = app
        .database
        .collection::<Document>("workflow_session_payloads")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{ciphertext:?}").contains("PRIVATE-WORKFLOW"));
    assert!(peer.state.requests.lock().unwrap().is_empty());
    app.cleanup().await;
}
#[tokio::test]
async fn session_branches_reject_cross_session_parents_and_deletion_hides_private_history() {
    let (app, _state, token, provider, _peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    let run = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    let first = session(&app, &token, &provider, &plan, "First session").await;
    let second = session(&app, &token, &provider, &plan, "Second session").await;
    let first_id = first["id"].as_str().unwrap();
    let second_id = second["id"].as_str().unwrap();
    let message=app.send("POST",&format!("/api/v1/workflows/sessions/{first_id}/messages"),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"parentMessageId":null,"runId":run["id"]}),Some(&token)).await;
    assert_eq!(message.0, StatusCode::CREATED);
    let message_id = message.1["data"]["id"].as_str().unwrap();
    assert_eq!(app.send("POST",&format!("/api/v1/workflows/sessions/{second_id}/messages"),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"parentMessageId":message_id,"runId":run["id"]}),Some(&token)).await.0,StatusCode::NOT_FOUND);
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/workflows/sessions/{first_id}?version=2"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/workflows/messages/{message_id}/content"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.cleanup().await;
}
#[tokio::test]
async fn session_audit_failure_rolls_back_encrypted_drafts() {
    use mongodb::bson::{Document, doc};
    let (app, _state, token, provider, _peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"workflows.sessions.create"}},"validationLevel":"strict"}).await.unwrap();
    let result=app.send("POST","/api/v1/workflows/sessions",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Failed session","selection":selection(&provider,&plan),"draft":inputs()}),Some(&token)).await;
    let mut counts = Vec::new();
    for collection in ["workflow_sessions", "workflow_session_payloads"] {
        counts.push(
            app.database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
        );
    }
    app.cleanup().await;
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(counts, vec![0, 0]);
}
#[tokio::test]
async fn expired_parent_intents_close_without_admitting_a_child() {
    use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
    let (app, state, token, provider, peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    let body = start(&provider, &plan);
    let run = submitted(&app, &token, "/api/v1/workflows/runs", body.clone()).await;
    let id = run["id"].as_str().unwrap();
    app.database
        .collection::<Document>("workflow_runs")
        .update_one(
            doc! {"_id":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"executionDeadline":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let workflow = worker::start_service(state.clone());
    let deadline = Instant::now() + Duration::from_secs(4);
    let observed = loop {
        let value = app
            .send(
                "GET",
                &format!("/api/v1/workflows/runs/{id}"),
                Value::Null,
                Some(&token),
            )
            .await
            .1["data"]
            .clone();
        if value["state"] != "pending" || Instant::now() >= deadline {
            break value;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    };
    worker::stop_service(&state);
    workflow.await.unwrap();
    let replay = submitted(&app, &token, "/api/v1/workflows/runs", body).await;
    let children = app
        .database
        .collection::<Document>("ai_runs")
        .count_documents(doc! {})
        .await
        .unwrap();
    let effects = peer.state.requests.lock().unwrap().len();
    app.cleanup().await;
    assert_eq!(observed["state"], "failed");
    assert_eq!(replay["id"], id);
    assert_eq!(children, 0);
    assert_eq!(effects, 0);
}
#[tokio::test]
async fn all_scope_metadata_does_not_grant_foreign_drafts_history_or_markdown_and_missing_keys_fail_closed()
 {
    use support::PASSWORD;
    let (mut app, _state, token, provider, _peer) = fixture(false).await;
    let plan = saved(&app, &token).await;
    let run = submitted(
        &app,
        &token,
        "/api/v1/workflows/runs",
        start(&provider, &plan),
    )
    .await;
    let run_id = run["id"].as_str().unwrap();
    let saved = session(&app, &token, &provider, &plan, "Owner session").await;
    let session_id = saved["id"].as_str().unwrap();
    let message=app.send("POST",&format!("/api/v1/workflows/sessions/{session_id}/messages"),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"parentMessageId":null,"runId":run_id}),Some(&token)).await;
    assert_eq!(message.0, StatusCode::CREATED);
    let message_id = message.1["data"]["id"].as_str().unwrap();
    let export = app
        .send(
            "POST",
            &format!("/api/v1/workflows/runs/{run_id}/exports"),
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1}),
            Some(&token),
        )
        .await;
    assert_eq!(export.0, StatusCode::CREATED);
    let export_uri = format!(
        "/api/v1/workflows/exports/{}/content",
        export.1["data"]["id"].as_str().unwrap()
    );
    let (status,role)=app.send("POST","/api/v1/roles",json!({"name":"Workflow all reader","slug":"workflow-all-reader","permissions":["workflows.read","workflows.write","workflows.invoke","ai.read","ai.invoke"],"dataScope":"all"}),Some(&token)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"workflow-all-reader","displayName":"Workflow reader","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(&token)).await.0,StatusCode::CREATED);
    let signed = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"workflow-all-reader","password":PASSWORD}),
            None,
        )
        .await;
    let foreign = signed.1["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/workflows/sessions?pageSize=1",
            Value::Null,
            Some(foreign)
        )
        .await
        .1["meta"]["total"],
        1
    );
    for path in [
        format!("/api/v1/workflows/sessions/{session_id}/content"),
        format!("/api/v1/workflows/messages/{message_id}/content"),
        format!("/api/v1/workflows/runs/{run_id}/content"),
    ] {
        assert_eq!(
            app.send("GET", &path, Value::Null, Some(foreign)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let history_metadata = app
        .send(
            "GET",
            &format!("/api/v1/workflows/sessions/{session_id}/messages?pageSize=1"),
            Value::Null,
            Some(foreign),
        )
        .await;
    assert_eq!(history_metadata.0, StatusCode::OK);
    assert_eq!(history_metadata.1["meta"]["total"], 1);
    assert_eq!(
        download(&app, foreign, &export_uri).await.0,
        StatusCode::NOT_FOUND
    );
    let other = session(&app, foreign, &provider, &plan, "Foreign own session").await;
    assert_eq!(app.send("POST",&format!("/api/v1/workflows/sessions/{}/messages",other["id"].as_str().unwrap()),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"parentMessageId":null,"runId":run_id}),Some(foreign)).await.0,StatusCode::NOT_FOUND);
    let original = app.router.clone();
    let no_keys = Config::from_lookup(|name| {
        if name == "TRI_JWT_SIGNING_KEY" {
            Some(support::KEY.into())
        } else {
            None
        }
    })
    .unwrap();
    app.router = build_router(AppState::new(no_keys, Some(app.database.clone())));
    for path in [
        format!("/api/v1/workflows/sessions/{session_id}/content"),
        format!("/api/v1/workflows/runs/{run_id}/content"),
    ] {
        let denied = app.send("GET", &path, Value::Null, Some(&token)).await;
        assert_eq!(denied.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(!denied.1.to_string().contains("PRIVATE-"));
    }
    let denied = download(&app, &token, &export_uri).await;
    assert_eq!(denied.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!String::from_utf8_lossy(&denied.2).contains("PRIVATE-"));
    app.router = original;
    app.cleanup().await;
}
