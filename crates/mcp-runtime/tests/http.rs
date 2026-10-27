#[path = "support/http_peer.rs"]
mod http_peer;
use http_peer::{Mode, Peer};
use serde_json::json;
use std::{sync::atomic::Ordering, time::Duration};
use tokio_util::sync::CancellationToken;
use tri_mcp_runtime::{
    http::{HttpSpec, connect},
    session::{Error, Limits},
};
fn limits() -> Limits {
    Limits {
        startup_timeout: Duration::from_secs(3),
        request_timeout: Duration::from_millis(300),
        stop_timeout: Duration::from_secs(1),
        ..Default::default()
    }
}
fn spec(peer: &Peer) -> HttpSpec {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        reqwest::header::HeaderValue::from_static("Bearer synthetic-http-fixture"),
    );
    HttpSpec {
        endpoint: peer.url.clone(),
        client,
        headers,
    }
}
#[tokio::test]
async fn actual_json_and_fragmented_sse_use_modern_metadata_without_legacy_requests() {
    for mode in [Mode::Json, Mode::Sse] {
        let peer = Peer::start(mode).await;
        let client = connect(spec(&peer), limits()).await.unwrap();
        let tools = client.tools(CancellationToken::new()).await.unwrap();
        assert_eq!(tools.len(), 2);
        let result = client
            .call("increment", json!({}), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.value["structuredContent"]["value"], 1);
        client.close().await;
        let seen = peer.seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].rpc, "server/discover");
        for request in seen.iter() {
            assert_eq!(request.method, "POST");
            assert_ne!(request.rpc, "initialize");
            assert_eq!(
                request
                    .headers
                    .get("mcp-protocol-version")
                    .map(String::as_str),
                Some("2026-07-28")
            );
            assert_eq!(request.headers.get("mcp-method"), Some(&request.rpc));
            assert_eq!(
                request.headers.get("authorization").map(String::as_str),
                Some("Bearer synthetic-http-fixture")
            );
            assert_eq!(
                request.body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
                "2026-07-28"
            );
        }
        assert_eq!(
            seen[2].headers.get("mcp-name").map(String::as_str),
            Some("increment")
        );
    }
}
#[tokio::test]
async fn lost_or_expired_call_responses_never_reinitialize_or_repeat_remote_effects() {
    for mode in [Mode::Lost, Mode::Expired] {
        let peer = Peer::start(mode).await;
        let client = connect(spec(&peer), limits()).await.unwrap();
        let error = client
            .call("increment", json!({}), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            if mode == Mode::Lost {
                Error::TransportUnknown
            } else {
                Error::RemoteUnknown { code: -32603 }
            }
        );
        assert!(!format!("{error:?} {error}").contains("synthetic-private-remote-error"));
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(peer.effects.load(Ordering::SeqCst), 1);
        {
            let seen = peer.seen.lock().unwrap();
            assert_eq!(seen.iter().filter(|v| v.rpc == "tools/call").count(), 1);
            assert_eq!(
                seen.iter().filter(|v| v.rpc == "server/discover").count(),
                1
            );
            assert!(
                seen.iter()
                    .all(|v| v.method == "POST" && v.rpc != "initialize")
            );
        }
        client.close().await;
    }
}
#[tokio::test]
async fn timeout_and_cancel_close_the_request_stream_and_preserve_unknown_effects() {
    let peer = Peer::start(Mode::Json).await;
    let client = connect(spec(&peer), limits()).await.unwrap();
    assert_eq!(
        client
            .call("hold", json!({}), CancellationToken::new())
            .await
            .unwrap_err(),
        Error::TimeoutUnknown
    );
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let (result, _) = tokio::join!(client.call("hold", json!({}), cancel), async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        trigger.cancel()
    });
    assert_eq!(result.unwrap_err(), Error::CancelledUnknown);
    tokio::time::timeout(Duration::from_secs(2), async {
        while peer.cancelled.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(peer.effects.load(Ordering::SeqCst), 2);
    client.close().await;
}
#[tokio::test]
async fn json_and_sse_byte_budgets_abort_untrusted_responses() {
    let peer = Peer::start(Mode::LargeJson).await;
    let small = Limits {
        max_frame_bytes: 4096,
        ..limits()
    };
    assert!(matches!(
        connect(spec(&peer), small.clone()).await,
        Err(Error::StartRejected)
    ));
    let peer = Peer::start(Mode::LargeSse).await;
    let client = connect(spec(&peer), small).await.unwrap();
    assert!(matches!(
        client
            .call("increment", json!({}), CancellationToken::new())
            .await,
        Err(Error::TransportUnknown)
    ));
    assert_eq!(peer.effects.load(Ordering::SeqCst), 1);
    client.close().await;
}
