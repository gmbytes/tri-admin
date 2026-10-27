use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tri_mcp_runtime::{
    process::{Phase, Process, ProgramSpec},
    session::{Error, Limits},
};
fn spec(scenario: &str, directory: &TempDir) -> ProgramSpec {
    let environment = BTreeMap::from([("TRI_FIXTURE_ALLOWED".into(), "explicit-only".into())]);
    #[cfg(windows)]
    let environment = {
        let mut environment = environment;
        environment.insert("SystemRoot".into(), std::env::var("SystemRoot").unwrap());
        environment
    };
    ProgramSpec {
        program: PathBuf::from(env!("CARGO_BIN_EXE_tri-mcp-test-peer")),
        args: vec![scenario.into(), directory.path().to_string_lossy().into()],
        cwd: directory.path().into(),
        environment,
    }
}
fn limits() -> Limits {
    Limits {
        startup_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_millis(250),
        stop_timeout: Duration::from_secs(3),
        ..Default::default()
    }
}
async fn wait_exit(process: &Process) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if process.state().phase == Phase::Exited {
                return;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn modern_discovery_pagination_calls_and_explicit_environment_are_real() {
    let directory = TempDir::new().unwrap();
    let mut process = Process::spawn(spec("pages", &directory), limits()).unwrap();
    let client = process.connect().await.unwrap();
    assert_eq!(process.state().phase, Phase::Running);
    let tools = client.tools(CancellationToken::new()).await.unwrap();
    assert_eq!(tools.len(), 6);
    let reply = client
        .call("increment", json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(reply.value["structuredContent"]["value"], 1);
    let inspected = client
        .call("inspect", json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        inspected.value["structuredContent"]["hasParentEnvironment"],
        false
    );
    assert_eq!(
        inspected.value["structuredContent"]["allowed"],
        "explicit-only"
    );
    process.stop().await.unwrap();
    assert_eq!(process.state().phase, Phase::Exited);
    let methods = std::fs::read_to_string(directory.path().join("methods.txt")).unwrap();
    assert!(methods.starts_with("server/discover\n"));
    assert!(!methods.lines().any(|line| line == "initialize"));
}
#[tokio::test]
async fn timeout_and_cancel_report_unknown_and_never_repeat_effects() {
    let directory = TempDir::new().unwrap();
    let mut process = Process::spawn(spec("normal", &directory), limits()).unwrap();
    let client = process.connect().await.unwrap();
    assert!(matches!(
        client
            .call("effect_then_wait", json!({}), CancellationToken::new())
            .await,
        Err(Error::TimeoutUnknown)
    ));
    let cancel = CancellationToken::new();
    let call = client.call("effect_then_wait", json!({}), cancel.clone());
    let trigger = async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        cancel.cancel();
    };
    let (result, _) = tokio::join!(call, trigger);
    assert!(matches!(result, Err(Error::CancelledUnknown)));
    let inspected = client
        .call("inspect", json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(inspected.value["structuredContent"]["value"], 2);
    assert_eq!(inspected.value["structuredContent"]["cancelled"], 2);
    process.stop().await.unwrap();
}
#[tokio::test]
async fn stopped_and_crashed_processes_terminate_their_real_descendants() {
    for crash in [false, true] {
        let directory = TempDir::new().unwrap();
        let mut process = Process::spawn(spec("tree", &directory), limits()).unwrap();
        let client = process.connect().await.unwrap();
        let inspect = client
            .call("inspect", json!({}), CancellationToken::new())
            .await
            .unwrap();
        let address = inspect.value["structuredContent"]["leaf"]["address"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "fixture leaf missing: {}",
                    String::from_utf8_lossy(&process.stderr().bytes)
                )
            });
        assert!(tokio::net::TcpStream::connect(address).await.is_ok());
        if crash {
            assert!(matches!(
                client
                    .call("crash", json!({}), CancellationToken::new())
                    .await,
                Err(Error::TransportUnknown)
            ));
            wait_exit(&process).await;
        } else {
            process.stop().await.unwrap();
        }
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
        assert_eq!(process.state().phase, Phase::Exited);
    }
}
#[tokio::test]
async fn failed_startups_are_reaped_and_never_fall_back_to_legacy() {
    for scenario in ["legacy", "legacy_error", "startup_hang", "oversized"] {
        let directory = TempDir::new().unwrap();
        let mut options = limits();
        options.startup_timeout = Duration::from_millis(300);
        let mut process = Process::spawn(spec(scenario, &directory), options).unwrap();
        assert!(process.connect().await.is_err(), "{scenario}");
        wait_exit(&process).await;
        if let Ok(methods) = std::fs::read_to_string(directory.path().join("methods.txt")) {
            assert!(!methods.lines().any(|line| line == "initialize"));
        }
    }
}
#[tokio::test]
async fn malformed_cursor_input_and_remote_errors_are_bounded_and_safe() {
    let directory = TempDir::new().unwrap();
    let mut process = Process::spawn(spec("cursor_loop", &directory), limits()).unwrap();
    let client = process.connect().await.unwrap();
    assert!(matches!(
        client.tools(CancellationToken::new()).await,
        Err(Error::Limit)
    ));
    assert!(matches!(
        client
            .call("bad\nname", json!({}), CancellationToken::new())
            .await,
        Err(Error::InvalidInput)
    ));
    let error = client
        .call("rpc_error", json!({}), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::RemoteRejected { code: -32602 }));
    assert!(!format!("{error:?} {error}").contains("SYNTHETIC-REMOTE"));
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        client.call("increment", Value::Null, token).await,
        Err(Error::CancelledBeforeSend) | Err(Error::InvalidInput)
    ));
    process.stop().await.unwrap();
}

#[tokio::test]
async fn stderr_is_drained_with_a_bound_and_input_required_does_not_repeat_the_call() {
    let directory = TempDir::new().unwrap();
    let mut options = limits();
    options.max_stderr_bytes = 1024;
    let mut process = Process::spawn(spec("stderr_flood", &directory), options).unwrap();
    let client = process.connect().await.unwrap();
    let result = client
        .call("need_input", json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(
        result.kind,
        tri_mcp_runtime::session::ReplyKind::InputRequired
    ));
    assert_eq!(result.value["requestState"], "opaque-fixture-state");
    let stderr = process.stderr();
    assert_eq!(stderr.bytes.len(), 1024);
    assert!(stderr.truncated);
    let methods = std::fs::read_to_string(directory.path().join("methods.txt")).unwrap();
    assert_eq!(
        methods.lines().filter(|line| *line == "tools/call").count(),
        1
    );
    process.stop().await.unwrap();
}

#[tokio::test]
async fn dropping_the_owner_confirms_descendant_exit_and_invalid_specs_do_not_spawn() {
    let directory = TempDir::new().unwrap();
    let mut invalid = spec("normal", &directory);
    invalid.program = PathBuf::from("relative-program");
    assert!(matches!(
        Process::spawn(invalid, limits()),
        Err(Error::InvalidInput)
    ));
    let mut process = Process::spawn(spec("tree", &directory), limits()).unwrap();
    let client = process.connect().await.unwrap();
    let result = client
        .call("inspect", json!({}), CancellationToken::new())
        .await
        .unwrap();
    let address = result.value["structuredContent"]["leaf"]["address"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut state = process.subscribe();
    drop(process);
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if state.borrow().phase == Phase::Exited {
                break;
            }
            state.changed().await.unwrap()
        }
    })
    .await
    .unwrap();
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
    assert!(matches!(
        client
            .call("increment", json!({}), CancellationToken::new())
            .await,
        Err(Error::Unavailable)
    ));
}

#[tokio::test]
async fn dropping_an_inflight_call_sends_cancellation_and_releases_the_slot() {
    let directory = TempDir::new().unwrap();
    let mut process = Process::spawn(spec("normal", &directory), limits()).unwrap();
    let client = std::sync::Arc::new(process.connect().await.unwrap());
    let caller = client.clone();
    let running = tokio::spawn(async move {
        caller
            .call("effect_then_wait", json!({}), CancellationToken::new())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let result = client
                .call("inspect", json!({}), CancellationToken::new())
                .await
                .unwrap();
            if result.value["structuredContent"]["value"] == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    running.abort();
    let _ = running.await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let result = client
                .call("inspect", json!({}), CancellationToken::new())
                .await
                .unwrap();
            if result.value["structuredContent"]["cancelled"] == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .unwrap();
    process.stop().await.unwrap();
}

#[tokio::test]
async fn busy_client_rejects_before_send_without_queueing_another_effect() {
    let directory = TempDir::new().unwrap();
    let mut options = limits();
    options.max_concurrent_calls = 1;
    let mut process = Process::spawn(spec("normal", &directory), options).unwrap();
    let client = std::sync::Arc::new(process.connect().await.unwrap());
    let caller = client.clone();
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let running =
        tokio::spawn(async move { caller.call("effect_then_wait", json!({}), signal).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if std::fs::read_to_string(directory.path().join("methods.txt"))
                .unwrap_or_default()
                .lines()
                .any(|method| method == "tools/call")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let result = client
        .call("increment", json!({}), CancellationToken::new())
        .await;
    assert!(
        matches!(result, Err(Error::BusyBeforeSend)),
        "busy connection must not queue another effect"
    );
    cancel.cancel();
    let _ = running.await;
    let inspect = client
        .call("inspect", json!({}), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(inspect.value["structuredContent"]["value"], 1);
    process.stop().await.unwrap();
}

#[tokio::test]
async fn fragmented_responses_survive_concurrent_outgoing_requests() {
    let directory = TempDir::new().unwrap();
    let mut process = Process::spawn(spec("partial", &directory), limits()).unwrap();
    let client = process.connect().await.unwrap();
    let (a, b, c) = tokio::join!(
        client.call("increment", json!({}), CancellationToken::new()),
        client.call("increment", json!({}), CancellationToken::new()),
        client.call("increment", json!({}), CancellationToken::new())
    );
    let mut values = [
        a.unwrap().value["structuredContent"]["value"]
            .as_i64()
            .unwrap(),
        b.unwrap().value["structuredContent"]["value"]
            .as_i64()
            .unwrap(),
        c.unwrap().value["structuredContent"]["value"]
            .as_i64()
            .unwrap(),
    ];
    values.sort();
    assert_eq!(values, [1, 2, 3]);
    process.stop().await.unwrap();
}
