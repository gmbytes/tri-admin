use axum::{
    body::Bytes,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Fault {
    None,
    DropAcknowledgement,
    LateWrite,
    RejectWrite,
    ExistingWithDeniedRead,
    RejectAfterRelease,
    CorruptRead,
    WriteAfterRelease,
}
struct State {
    upstream: String,
    client: reqwest::Client,
    fault: AtomicUsize,
    pub puts: AtomicUsize,
    pub reads: AtomicUsize,
    release: CancellationToken,
    committed: CancellationToken,
    received: CancellationToken,
}
pub struct S3FaultProxy {
    pub endpoint: String,
    state: Arc<State>,
    server: JoinHandle<()>,
}
impl S3FaultProxy {
    pub async fn start(fixture: &Value, fault: Fault) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(State {
            upstream: fixture["endpoint"].as_str().unwrap().to_owned(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            fault: AtomicUsize::new(fault as usize),
            puts: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            release: CancellationToken::new(),
            committed: CancellationToken::new(),
            received: CancellationToken::new(),
        });
        let shared = state.clone();
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let shared = shared.clone();
                        connections.spawn(async move { handle(socket, shared).await });
                    }
                    result = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            endpoint,
            state,
            server,
        }
    }
    pub fn set_fault(&self, fault: Fault) {
        self.state.fault.store(fault as usize, Ordering::SeqCst);
    }
    pub fn puts(&self) -> usize {
        self.state.puts.load(Ordering::SeqCst)
    }
    pub fn reads(&self) -> usize {
        self.state.reads.load(Ordering::SeqCst)
    }
    pub async fn wait_for_write(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.state.received.cancelled(),
        )
        .await
        .unwrap();
    }
    pub fn release(&self) {
        self.state.release.cancel();
    }
    pub async fn release_late_write(&self) {
        self.release();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.state.committed.cancelled(),
        )
        .await
        .unwrap();
    }
}
impl Drop for S3FaultProxy {
    fn drop(&mut self) {
        self.state.release.cancel();
        self.server.abort();
    }
}
struct Incoming {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Bytes,
}
async fn incoming(socket: &mut TcpStream) -> Incoming {
    let mut data = Vec::new();
    let header_end = loop {
        if let Some(end) = data.windows(4).position(|value| value == b"\r\n\r\n") {
            break end + 4;
        }
        assert!(data.len() < 64 * 1024, "oversized fixture HTTP headers");
        let mut buffer = [0; 4096];
        let size = socket.read(&mut buffer).await.unwrap();
        assert!(size > 0, "fixture request closed before headers");
        data.extend_from_slice(&buffer[..size]);
    };
    let header = std::str::from_utf8(&data[..header_end]).unwrap();
    let mut lines = header.split("\r\n");
    let mut start = lines.next().unwrap().split(' ');
    let method = Method::from_bytes(start.next().unwrap().as_bytes()).unwrap();
    let path = start.next().unwrap().to_owned();
    let mut headers = HeaderMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').unwrap();
        headers.append(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value.trim()).unwrap(),
        );
    }
    assert!(
        !headers.contains_key("transfer-encoding"),
        "fixture requires fixed-size bodies"
    );
    let length = headers
        .get("content-length")
        .map(|v| v.to_str().unwrap().parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(length <= 2 * 1024 * 1024, "oversized fixture body");
    while data.len() < header_end + length {
        let mut buffer = [0; 8192];
        let size = socket.read(&mut buffer).await.unwrap();
        assert!(size > 0, "fixture request closed before body");
        data.extend_from_slice(&buffer[..size]);
    }
    Incoming {
        method,
        path,
        headers,
        body: Bytes::copy_from_slice(&data[header_end..header_end + length]),
    }
}
async fn forward(state: &State, request: &Incoming) -> (StatusCode, Bytes) {
    // Preserve the signed Host and all x-amz headers when forwarding to the real S3 fixture.
    let mut headers = request.headers.clone();
    headers.remove("connection");
    let response = state
        .client
        .request(
            request.method.clone(),
            format!("{}{}", state.upstream, request.path),
        )
        .headers(headers)
        .body(request.body.clone())
        .send()
        .await
        .expect("fixture upstream request failed");
    (
        response.status(),
        response
            .bytes()
            .await
            .expect("fixture upstream body failed"),
    )
}
async fn respond(socket: &mut TcpStream, code: StatusCode, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        code.as_u16(),
        code.canonical_reason().unwrap_or("Fixture"),
        body.len()
    );
    let _ = socket.write_all(header.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.shutdown().await;
}
async fn handle(mut socket: TcpStream, state: Arc<State>) {
    let request = incoming(&mut socket).await;
    let candidate = request.path.contains("/objects/") && !request.path.contains("/_tri-probes/");
    let fault = state.fault.load(Ordering::SeqCst);
    if candidate && request.method == Method::PUT {
        state.puts.fetch_add(1, Ordering::SeqCst);
        state.received.cancel();
        if fault == Fault::WriteAfterRelease as usize {
            state.release.cancelled().await;
            let (code, body) = forward(&state, &request).await;
            assert!(code.is_success(), "S3 write after process death failed");
            state.committed.cancel();
            respond(&mut socket, code, &body).await;
            return;
        }
        if fault == Fault::RejectAfterRelease as usize {
            state.release.cancelled().await;
        }
        if fault == Fault::RejectWrite as usize || fault == Fault::RejectAfterRelease as usize {
            respond(
                &mut socket,
                StatusCode::FORBIDDEN,
                b"private-storage-denial",
            )
            .await;
            return;
        }
        if fault == Fault::LateWrite as usize {
            respond(
                &mut socket,
                StatusCode::SERVICE_UNAVAILABLE,
                b"private-deferred-write",
            )
            .await;
            state.release.cancelled().await;
            let (code, _) = forward(&state, &request).await;
            assert!(code.is_success(), "late S3 write failed");
            state.committed.cancel();
            return;
        }
        if fault == Fault::DropAcknowledgement as usize
            || fault == Fault::ExistingWithDeniedRead as usize
        {
            let (code, _) = forward(&state, &request).await;
            assert!(code.is_success(), "S3 write before fault failed");
            state.committed.cancel();
            if fault == Fault::DropAcknowledgement as usize {
                return;
            }
            // Replaying the immutable request produces a genuine S3 412 response.
            let (code, body) = forward(&state, &request).await;
            assert_eq!(code, StatusCode::PRECONDITION_FAILED);
            respond(&mut socket, code, &body).await;
            return;
        }
    }
    if candidate && request.method == Method::GET {
        state.reads.fetch_add(1, Ordering::SeqCst);
        if fault == Fault::ExistingWithDeniedRead as usize {
            respond(&mut socket, StatusCode::FORBIDDEN, b"private-read-denial").await;
            return;
        }
    }
    let (code, mut body) = forward(&state, &request).await;
    if candidate
        && request.method == Method::GET
        && fault == Fault::CorruptRead as usize
        && code.is_success()
        && !body.is_empty()
    {
        let mut corrupted = body.to_vec();
        corrupted[0] ^= 1;
        body = Bytes::from(corrupted);
    }
    respond(&mut socket, code, &body).await;
}
