use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tri_ai_runtime::{
    Limits, ResponseState, Status,
    http::{Adapter, CreateRequest, FailureKind, HttpSpec, Policy, Reference},
};

#[derive(Clone)]
struct Fixture {
    mode: &'static str,
    seen: Arc<Mutex<Vec<Captured>>>,
}
struct Captured {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Value,
}
fn snapshot(status: &str, text: &str) -> Value {
    json!({"id":"resp_fixture","object":"response","status":status,"output":if text.is_empty(){json!([])}else{json!([{"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}])}})
}
fn wire(value: Value) -> String {
    format!("data: {value}\r\n\r\n")
}
async fn serve(State(fixture): State<Fixture>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 100_000).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    fixture.seen.lock().unwrap().push(Captured {
        method: parts.method,
        path: parts.uri.to_string(),
        headers: parts.headers,
        body: value,
    });
    match fixture.mode {
        "json" => (
            [("content-type", "application/json")],
            snapshot("completed", "JSON reply").to_string(),
        )
            .into_response(),
        "queued" => (
            [("content-type", "application/json")],
            snapshot("queued", "").to_string(),
        )
            .into_response(),
        "cancel" => (
            [("content-type", "application/json")],
            snapshot("cancelled", "").to_string(),
        )
            .into_response(),
        "redirect" => (
            StatusCode::FOUND,
            [("location", "/credential-trap")],
            "PRIVATE-ERROR-BODY",
        )
            .into_response(),
        "failure" => (StatusCode::INTERNAL_SERVER_ERROR, "PRIVATE-ERROR-BODY").into_response(),
        "hang" => {
            tokio::time::sleep(Duration::from_secs(1)).await;
            (StatusCode::OK, "{}").into_response()
        }
        "large" => (
            [("content-type", "application/json")],
            Body::from_stream(stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from(
                "x".repeat(300),
            ))])),
        )
            .into_response(),
        "idle" => {
            let first = wire(
                json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_fixture","status":"in_progress"}}),
            );
            let source =
                stream::once(async move { Ok::<_, std::io::Error>(bytes::Bytes::from(first)) })
                    .chain(stream::once(async {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b": waiting\n\n"))
                    }));
            (
                [("content-type", "text/event-stream")],
                Body::from_stream(source),
            )
                .into_response()
        }
        mode => {
            let mut data = if mode == "resume" {
                wire(json!({"type":"keepalive","sequence_number":999}))
            } else {
                wire(
                    json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_fixture","status":"in_progress"}}),
                )
            };
            data.push_str(&wire(json!({"type":"response.output_text.delta","sequence_number":1,"output_index":0,"content_index":0,"item_id":"msg_fixture","delta":"流式 reply"})));
            if mode != "truncated" {
                data.push_str(&wire(json!({"type":"response.completed","sequence_number":2,"response":snapshot("completed","流式 reply")})));
            }
            if mode == "buffered" {
                return ([("content-type", "text/event-stream")], data).into_response();
            }
            let chunks = data
                .as_bytes()
                .chunks(7)
                .map(|bytes| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(bytes)))
                .collect::<Vec<_>>();
            (
                [("content-type", "text/event-stream")],
                Body::from_stream(stream::iter(chunks)),
            )
                .into_response()
        }
    }
}

#[tokio::test]
async fn cancellation_and_total_deadline_are_checked_between_already_buffered_events() {
    for cancel_read in [true, false] {
        let server = start_server("buffered").await;
        let adapter = make_adapter(
            &server,
            Limits::default(),
            Policy {
                total_timeout: Duration::from_millis(150),
                idle_timeout: Duration::from_millis(150),
            },
        );
        let token = CancellationToken::new();
        let mut exchange = adapter.create(request(true), token.clone()).await.unwrap();
        exchange.next().await.unwrap();
        if cancel_read {
            token.cancel();
        } else {
            tokio::time::sleep(Duration::from_millis(180)).await;
        }
        let error = exchange
            .next()
            .await
            .err()
            .expect("buffered events must obey cancellation and total deadline");
        assert_eq!(
            error.kind,
            if cancel_read {
                FailureKind::Cancelled
            } else {
                FailureKind::Timeout
            }
        );
        assert_eq!(exchange.state().sequence(), Some(0));
        assert_eq!(server.fixture.seen.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn host_tightened_limits_apply_to_restored_states_before_accepting_output() {
    let server = start_server("json").await;
    let adapter = make_adapter(
        &server,
        Limits {
            max_text_bytes: 4,
            ..Limits::default()
        },
        Policy::default(),
    );
    let reference = Reference::new("resp_fixture", true, true).unwrap();
    let mut exchange = adapter
        .retrieve(
            &reference,
            ResponseState::new(Limits::default()).unwrap(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        exchange.next().await.err().unwrap().kind,
        FailureKind::Protocol(tri_ai_runtime::Error::Limit)
    );
}
#[tokio::test]
async fn idle_deadline_also_bounds_waiting_for_response_headers() {
    let server = start_server("hang").await;
    let adapter = make_adapter(
        &server,
        Limits::default(),
        Policy {
            total_timeout: Duration::from_secs(1),
            idle_timeout: Duration::from_millis(60),
        },
    );
    let started = std::time::Instant::now();
    let error = adapter
        .create(request(false), CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind, FailureKind::Timeout);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(server.fixture.seen.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn cancellation_after_creation_and_idle_body_timeout_retain_known_response_for_recovery() {
    for cancel_read in [true, false] {
        let server = start_server("idle").await;
        let adapter = make_adapter(
            &server,
            Limits::default(),
            Policy {
                total_timeout: Duration::from_secs(2),
                idle_timeout: Duration::from_millis(80),
            },
        );
        let token = CancellationToken::new();
        let mut exchange = adapter.create(request(true), token.clone()).await.unwrap();
        exchange.next().await.unwrap();
        if cancel_read {
            token.cancel();
        }
        let error = exchange.next().await.err().unwrap();
        assert_eq!(
            error.kind,
            if cancel_read {
                FailureKind::Cancelled
            } else {
                FailureKind::Timeout
            }
        );
        assert!(error.may_have_been_sent);
        assert_eq!(exchange.state().response_id(), Some("resp_fixture"));
        assert_eq!(server.fixture.seen.lock().unwrap().len(), 1);
    }
}
struct Server {
    endpoint: String,
    fixture: Fixture,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn start_server(mode: &'static str) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/responses", listener.local_addr().unwrap());
    let fixture = Fixture {
        mode,
        seen: Default::default(),
    };
    let router = Router::new().fallback(serve).with_state(fixture.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server {
        endpoint,
        fixture,
        task,
    }
}
fn request(stream: bool) -> CreateRequest {
    CreateRequest {
        model: "fixture-model".into(),
        input: "PRIVATE-PROMPT".into(),
        instructions: Some("PRIVATE-INSTRUCTIONS".into()),
        max_output_tokens: 128,
        background: true,
        stream,
    }
}
fn make_adapter(server: &Server, limits: Limits, policy: Policy) -> Adapter {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        "Bearer PRIVATE-CREDENTIAL".parse().unwrap(),
    );
    Adapter::new(
        HttpSpec {
            endpoint: server.endpoint.clone(),
            client,
            headers,
        },
        limits,
        policy,
    )
    .unwrap()
}
#[tokio::test]
async fn real_json_creation_posts_one_typed_request_and_validates_terminal_output() {
    let server = start_server("json").await;
    let adapter = make_adapter(&server, Limits::default(), Policy::default());
    let mut exchange = adapter
        .create(request(false), CancellationToken::new())
        .await
        .unwrap();
    assert!(exchange.next().await.unwrap().is_some());
    assert!(exchange.next().await.unwrap().is_none());
    assert_eq!(exchange.state().sequence(), None);
    let reply = exchange.into_state().finish().unwrap();
    assert_eq!(reply.status, Status::Completed);
    assert_eq!(reply.text(), "JSON reply");
    let seen = server.fixture.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, Method::POST);
    assert_eq!(seen[0].path, "/v1/responses");
    assert_eq!(
        seen[0].headers["authorization"],
        "Bearer PRIVATE-CREDENTIAL"
    );
    assert_eq!(
        seen[0].body,
        json!({"model":"fixture-model","input":"PRIVATE-PROMPT","instructions":"PRIVATE-INSTRUCTIONS","max_output_tokens":128,"background":true,"stream":false,"store":true,"tools":[]})
    );
}
#[tokio::test]
async fn fragmented_real_stream_yields_private_checkpoints_and_complete_text() {
    let server = start_server("stream").await;
    let adapter = make_adapter(&server, Limits::default(), Policy::default());
    let mut exchange = adapter
        .create(request(true), CancellationToken::new())
        .await
        .unwrap();
    let mut count = 0;
    while exchange.next().await.unwrap().is_some() {
        count += 1;
        let state =
            ResponseState::restore(&exchange.state().checkpoint().unwrap(), Limits::default())
                .unwrap();
        assert_eq!(state.sequence(), Some(count - 1));
    }
    assert_eq!(count, 3);
    assert_eq!(exchange.into_state().finish().unwrap().text(), "流式 reply");
    assert_eq!(server.fixture.seen.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn truncated_stream_preserves_response_identity_without_reposting() {
    let server = start_server("truncated").await;
    let adapter = make_adapter(&server, Limits::default(), Policy::default());
    let mut exchange = adapter
        .create(request(true), CancellationToken::new())
        .await
        .unwrap();
    assert!(exchange.next().await.unwrap().is_some());
    assert!(exchange.next().await.unwrap().is_some());
    let error = exchange.next().await.err().unwrap();
    assert!(error.may_have_been_sent);
    assert_eq!(
        error.kind,
        FailureKind::Protocol(tri_ai_runtime::Error::Truncated)
    );
    assert_eq!(exchange.state().response_id(), Some("resp_fixture"));
    assert_eq!(server.fixture.seen.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn background_resume_gets_only_saved_cursor_and_keepalive_does_not_advance_it() {
    let server = start_server("resume").await;
    let adapter = make_adapter(&server, Limits::default(), Policy::default());
    let mut state = ResponseState::new(Limits::default()).unwrap();
    state.apply_json(&serde_json::to_vec(&json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_fixture","status":"in_progress"}})).unwrap()).unwrap();
    let reference = Reference::new("resp_fixture", true, true).unwrap();
    let mut exchange = adapter
        .resume(&reference, state, CancellationToken::new())
        .await
        .unwrap();
    exchange.next().await.unwrap();
    assert_eq!(exchange.state().sequence(), Some(0));
    while exchange.next().await.unwrap().is_some() {}
    assert_eq!(exchange.into_state().finish().unwrap().text(), "流式 reply");
    let seen = server.fixture.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, Method::GET);
    assert_eq!(
        seen[0].path,
        "/v1/responses/resp_fixture?stream=true&starting_after=0"
    );
    assert_eq!(seen[0].body, Value::Null);
}
#[tokio::test]
async fn background_json_retrieve_and_cancel_use_safe_known_id_paths() {
    for (mode, cancel, path) in [
        ("queued", false, "/v1/responses/resp_fixture"),
        ("cancel", true, "/v1/responses/resp_fixture/cancel"),
    ] {
        let server = start_server(mode).await;
        let adapter = make_adapter(&server, Limits::default(), Policy::default());
        let reference = Reference::new("resp_fixture", true, true).unwrap();
        let state = ResponseState::new(Limits::default()).unwrap();
        let mut exchange = if cancel {
            adapter
                .cancel(&reference, state, CancellationToken::new())
                .await
                .unwrap()
        } else {
            adapter
                .retrieve(&reference, state, CancellationToken::new())
                .await
                .unwrap()
        };
        exchange.next().await.unwrap();
        assert!(exchange.next().await.unwrap().is_none());
        let seen = server.fixture.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].method,
            if cancel { Method::POST } else { Method::GET }
        );
        assert_eq!(seen[0].path, path);
    }
}
#[tokio::test]
async fn cancellation_before_send_and_invalid_request_never_reach_upstream() {
    let server = start_server("json").await;
    let adapter = make_adapter(&server, Limits::default(), Policy::default());
    let token = CancellationToken::new();
    token.cancel();
    let error = adapter.create(request(false), token).await.err().unwrap();
    assert!(!error.may_have_been_sent);
    assert_eq!(error.kind, FailureKind::Cancelled);
    let mut invalid = request(false);
    invalid.input = "x".repeat(65537);
    assert_eq!(
        adapter
            .create(invalid, CancellationToken::new())
            .await
            .err()
            .unwrap()
            .kind,
        FailureKind::Input
    );
    let reference = Reference::new("resp_fixture", false, false).unwrap();
    assert!(
        adapter
            .cancel(
                &reference,
                ResponseState::new(Limits::default()).unwrap(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert!(Reference::new("../escape", true, true).is_err());
    assert!(server.fixture.seen.lock().unwrap().is_empty());
}
#[tokio::test]
async fn redirects_and_provider_error_bodies_are_never_followed_or_exposed() {
    for (mode, status) in [("redirect", 302), ("failure", 500)] {
        let server = start_server(mode).await;
        let adapter = make_adapter(&server, Limits::default(), Policy::default());
        let error = adapter
            .create(request(false), CancellationToken::new())
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, FailureKind::Status(status));
        assert!(error.may_have_been_sent);
        assert!(!format!("{error:?} {error}").contains("PRIVATE"));
        assert_eq!(server.fixture.seen.lock().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn total_deadline_is_unknown_after_single_submission_and_json_is_bounded() {
    let server = start_server("hang").await;
    let adapter = make_adapter(
        &server,
        Limits::default(),
        Policy {
            total_timeout: Duration::from_millis(80),
            idle_timeout: Duration::from_millis(80),
        },
    );
    let error = adapter
        .create(request(false), CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind, FailureKind::Timeout);
    assert!(error.may_have_been_sent);
    assert_eq!(server.fixture.seen.lock().unwrap().len(), 1);
    let server = start_server("large").await;
    let adapter = make_adapter(
        &server,
        Limits {
            max_stream_bytes: 256,
            ..Limits::default()
        },
        Policy::default(),
    );
    let mut exchange = adapter
        .create(request(false), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        exchange.next().await.err().unwrap().kind,
        FailureKind::Protocol(tri_ai_runtime::Error::Limit)
    );
}
#[tokio::test]
async fn resumed_reads_keep_the_cursor_but_start_a_new_wire_byte_budget() {
    let server = start_server("resume").await;
    let limits = Limits {
        max_stream_bytes: 1024,
        ..Limits::default()
    };
    let adapter = make_adapter(&server, limits, Policy::default());
    let mut state = ResponseState::new(limits).unwrap();
    state.apply_json(&serde_json::to_vec(&json!({"type":"response.created","sequence_number":0,"response":{"id":"resp_fixture","status":"in_progress"},"padding":"x".repeat(700)})).unwrap()).unwrap();
    let reference = Reference::new("resp_fixture", true, true).unwrap();
    let mut exchange = adapter
        .resume(&reference, state, CancellationToken::new())
        .await
        .unwrap();
    while exchange.next().await.unwrap().is_some() {}
    assert_eq!(exchange.into_state().finish().unwrap().text(), "流式 reply");
    assert_eq!(server.fixture.seen.lock().unwrap().len(), 1);
}
