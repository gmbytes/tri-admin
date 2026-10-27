use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Json,
    Sse,
    Lost,
    Expired,
    LargeJson,
    LargeSse,
}
#[derive(Clone)]
pub struct Seen {
    pub method: String,
    pub rpc: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}
pub struct Peer {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    pub effects: Arc<AtomicUsize>,
    pub cancelled: Arc<AtomicUsize>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}
impl Peer {
    pub async fn start(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(vec![]));
        let effects = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicUsize::new(0));
        let stop = CancellationToken::new();
        let (records, count, cancels, shutdown) = (
            seen.clone(),
            effects.clone(),
            cancelled.clone(),
            stop.clone(),
        );
        let task = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{let Ok((stream,_))=accepted else{break};let (records,count,cancels)=(records.clone(),count.clone(),cancels.clone());handlers.spawn(async move{let _=serve(stream,mode,records,count,cancels).await;});},
                    _=shutdown.cancelled()=>break,
                    _=handlers.join_next(),if !handlers.is_empty()=>{}
                }
            }
        });
        Self {
            url,
            seen,
            effects,
            cancelled,
            stop,
            task,
        }
    }
}
async fn serve(
    mut stream: TcpStream,
    mode: Mode,
    seen: Arc<Mutex<Vec<Seen>>>,
    effects: Arc<AtomicUsize>,
    cancelled: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        bytes.push(stream.read_u8().await?);
        if bytes.len() > 16384 {
            return Ok(());
        }
    }
    let text = String::from_utf8(bytes).unwrap();
    let mut lines = text.split("\r\n");
    let method = lines.next().unwrap().split(' ').next().unwrap().to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if length > 1024 * 1024 {
        return Ok(());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let rpc = body["method"].as_str().unwrap_or("").to_owned();
    seen.lock().unwrap().push(Seen {
        method: method.clone(),
        rpc: rpc.clone(),
        headers,
        body: body.clone(),
    });
    if method != "POST" {
        return reply(&mut stream, 405, "application/json", b"{}", false).await;
    }
    let id = body["id"].clone();
    let result = match rpc.as_str() {
        "server/discover" => {
            let mut result = json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"});
            if mode == Mode::LargeJson {
                result["instructions"] = json!("x".repeat(32 * 1024));
            }
            result
        }
        "tools/list" => {
            json!({"resultType":"complete","tools":[{"name":"increment","inputSchema":{"type":"object"}},{"name":"hold","inputSchema":{"type":"object"}}],"ttlMs":0,"cacheScope":"private"})
        }
        "tools/call" => {
            let value = effects.fetch_add(1, Ordering::SeqCst) + 1;
            if mode == Mode::Lost {
                return Ok(());
            }
            if mode == Mode::Expired {
                let bytes=serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"synthetic-private-remote-error"}})).unwrap();
                return reply(&mut stream, 404, "application/json", &bytes, false).await;
            }
            if body["params"]["name"] == "hold" {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 256\r\nConnection: close\r\n\r\n: pending\n\n").await?;
                stream.flush().await?;
                let mut byte = [0];
                if matches!(
                    tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte)).await,
                    Ok(Ok(0))
                ) {
                    cancelled.fetch_add(1, Ordering::SeqCst);
                }
                return Ok(());
            }
            json!({"resultType":"complete","content":[],"structuredContent":{"value":value},"isError":false})
        }
        _ => return reply(&mut stream, 400, "application/json", b"{}", false).await,
    };
    let mut bytes = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":result})).unwrap();
    let sse = mode == Mode::Sse || (mode == Mode::LargeSse && rpc == "tools/call");
    if sse {
        bytes = if mode == Mode::LargeSse {
            format!("event: message\ndata: {}\n\n", "x".repeat(32 * 1024)).into_bytes()
        } else {
            format!(
                "event: message\ndata: {}\n\n",
                String::from_utf8(bytes).unwrap()
            )
            .into_bytes()
        };
    }
    if mode == Mode::LargeJson || mode == Mode::LargeSse && rpc == "tools/call" {
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
                    if sse {
                        "text/event-stream"
                    } else {
                        "application/json"
                    }
                )
                .as_bytes(),
            )
            .await?;
        for chunk in bytes.chunks(512) {
            stream.write_all(chunk).await?;
            stream.flush().await?;
        }
        return stream.shutdown().await;
    }
    reply(
        &mut stream,
        200,
        if sse {
            "text/event-stream"
        } else {
            "application/json"
        },
        &bytes,
        sse,
    )
    .await
}
async fn reply(
    stream: &mut TcpStream,
    status: u16,
    kind: &str,
    bytes: &[u8],
    fragment: bool,
) -> std::io::Result<()> {
    stream.write_all(format!("HTTP/1.1 {status} Reply\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len()).as_bytes()).await?;
    if fragment {
        // Exercise partial events without accumulating timer rounding per seven bytes.
        for chunk in bytes.chunks((bytes.len() / 3).max(1)) {
            stream.write_all(chunk).await?;
            stream.flush().await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    } else {
        stream.write_all(bytes).await?;
    }
    stream.shutdown().await
}
