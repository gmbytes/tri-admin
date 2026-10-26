use rmcp::model::{ClientJsonRpcMessage, GetMeta, ProtocolVersion};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
static PARTIAL: AtomicBool = AtomicBool::new(false);
async fn send(output: &Arc<Mutex<tokio::io::Stdout>>, value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    let mut output = output.lock().await;
    if PARTIAL.load(Ordering::SeqCst) {
        let at = bytes.len() / 2;
        output.write_all(&bytes[..at]).await.unwrap();
        output.flush().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        output.write_all(&bytes[at..]).await.unwrap();
    } else {
        output.write_all(&bytes).await.unwrap();
    }
    output.flush().await.unwrap();
}
#[tokio::main(worker_threads = 2)]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let scenario = args.first().map(String::as_str).unwrap_or("normal");
    PARTIAL.store(scenario == "partial", Ordering::SeqCst);
    let directory = PathBuf::from(args.get(1).unwrap());
    if scenario == "leaf" {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        tokio::fs::write(directory.join("leaf.json"),serde_json::to_vec(&json!({"pid":std::process::id(),"address":listener.local_addr().unwrap().to_string()})).unwrap()).await.unwrap();
        let _keep_listener = listener;
        std::future::pending::<()>().await;
        return;
    }
    if scenario == "startup_hang" {
        std::future::pending::<()>().await;
        return;
    }
    if scenario == "oversized" {
        let mut out = tokio::io::stdout();
        out.write_all(&vec![b'x'; 1024 * 1024]).await.unwrap();
        out.flush().await.unwrap();
        std::future::pending::<()>().await;
        return;
    }
    if scenario == "stderr_flood" {
        tokio::io::stderr()
            .write_all(&vec![b'e'; 100 * 1024])
            .await
            .unwrap();
    }
    let mut child = None;
    if scenario == "tree" {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["leaf", directory.to_str().unwrap()])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            command.creation_flags(0x08000000);
            command.env("SystemRoot", std::env::var_os("SystemRoot").unwrap());
        }
        child = Some(command.spawn().unwrap());
        let mut child_stderr = child.as_mut().unwrap().stderr.take().unwrap();
        let diagnostics = directory.join("leaf.stderr");
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut data = Vec::new();
            let _ = child_stderr.read_to_end(&mut data).await;
            let _ = tokio::fs::write(diagnostics, data).await;
        });
        for _ in 0..100 {
            if directory.join("leaf.json").exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        if !directory.join("leaf.json").exists() {
            let diagnostic = tokio::fs::read_to_string(directory.join("leaf.stderr"))
                .await
                .unwrap_or_default();
            eprintln!("fixture leaf not ready: {diagnostic}");
        }
    }
    let output = Arc::new(Mutex::new(tokio::io::stdout()));
    let count = Arc::new(AtomicI64::new(0));
    let cancelled = Arc::new(AtomicI64::new(0));
    let pending = Arc::new(Mutex::new(HashMap::<String, CancellationToken>::new()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        let raw: Value = serde_json::from_str(&line).unwrap();
        let method = raw["method"].as_str().unwrap_or("");
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("methods.txt"))
            .await
            .unwrap()
            .write_all(format!("{method}\n").as_bytes())
            .await
            .unwrap();
        if method == "initialize" {
            std::process::exit(91)
        }
        if method == "notifications/cancelled" {
            let id = raw["params"]["requestId"].to_string();
            if let Some(token) = pending.lock().await.remove(&id) {
                cancelled.fetch_add(1, Ordering::SeqCst);
                token.cancel();
            }
            continue;
        }
        if raw.get("id").is_none() {
            continue;
        }
        let id = raw["id"].clone();
        let parsed: ClientJsonRpcMessage = serde_json::from_value(raw.clone()).unwrap();
        if let ClientJsonRpcMessage::Request(request) = parsed {
            assert_eq!(
                request.request.get_meta().protocol_version(),
                Some(ProtocolVersion::V_2026_07_28)
            );
        }
        if method == "server/discover" {
            if scenario == "legacy_error" {
                send(
                    &output,
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"not found"}}),
                )
                .await;
                continue;
            }
            send(&output,json!({"jsonrpc":"2.0","id":id,"result":{"resultType":"complete","supportedVersions":[if scenario=="legacy"{"2025-06-18"}else{"2026-07-28"}],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"}})).await;
            continue;
        }
        if method == "tools/list" {
            let names = if raw["params"]["cursor"].is_null() {
                vec![
                    "increment",
                    "inspect",
                    "effect_then_wait",
                    "crash",
                    "rpc_error",
                ]
            } else {
                vec!["last"]
            };
            let next = if scenario == "cursor_loop" {
                Some("repeat")
            } else if scenario == "pages" && raw["params"]["cursor"].is_null() {
                Some("next")
            } else {
                None
            };
            let tools=names.into_iter().map(|name|json!({"name":name,"description":"Test operation","inputSchema":{"type":"object","properties":{}}})).collect::<Vec<_>>();
            send(&output,json!({"jsonrpc":"2.0","id":id,"result":{"resultType":"complete","tools":tools,"nextCursor":next,"ttlMs":0,"cacheScope":"private"}})).await;
            continue;
        }
        if method == "tools/call" {
            let name = raw["params"]["name"].as_str().unwrap_or("");
            if name == "need_input" {
                send(&output,json!({"jsonrpc":"2.0","id":id,"result":{"resultType":"input_required","requestState":"opaque-fixture-state"}})).await;
                continue;
            }
            if name == "crash" {
                std::process::exit(23)
            }
            if name == "rpc_error" {
                send(&output,json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"SYNTHETIC-REMOTE-DETAIL-MUST-NOT-LEAK"}})).await;
                continue;
            }
            if name == "effect_then_wait" {
                let value = count.fetch_add(1, Ordering::SeqCst) + 1;
                let token = CancellationToken::new();
                pending.lock().await.insert(id.to_string(), token.clone());
                let output = output.clone();
                tokio::spawn(async move {
                    tokio::select! {_ = token.cancelled()=>{},_ = tokio::time::sleep(std::time::Duration::from_secs(8))=>{send(&output,json!({"jsonrpc":"2.0","id":id,"result":{"resultType":"complete","content":[],"structuredContent":{"value":value},"isError":false}})).await;}}
                });
                continue;
            }
            let value = if name == "increment" {
                count.fetch_add(1, Ordering::SeqCst) + 1
            } else {
                count.load(Ordering::SeqCst)
            };
            let leaf = tokio::fs::read(directory.join("leaf.json"))
                .await
                .ok()
                .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap());
            let result = json!({"value":value,"cancelled":cancelled.load(Ordering::SeqCst),"hasParentEnvironment":std::env::var_os("USERPROFILE").is_some()||std::env::var_os("HOME").is_some()||std::env::var_os("TRI_JWT_SIGNING_KEY").is_some(),"allowed":std::env::var("TRI_FIXTURE_ALLOWED").ok(),"leaf":leaf});
            send(&output,json!({"jsonrpc":"2.0","id":id,"result":{"resultType":"complete","content":[],"structuredContent":result,"isError":false}})).await;
        } else {
            send(
                &output,
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported"}}),
            )
            .await;
        }
    }
    drop(child);
}
