mod support;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::any,
};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use support::TestApp;

#[derive(Clone)]
struct HttpState {
    calls: Arc<AtomicUsize>,
    seen: Arc<Mutex<HeaderMap>>,
    status: StatusCode,
    location: Option<String>,
    delay: Duration,
}
struct HttpFixture {
    url: String,
    state: HttpState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn fixture(status: StatusCode, location: Option<String>, delay: Duration) -> HttpFixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = HttpState {
        calls: Arc::new(AtomicUsize::new(0)),
        seen: Arc::new(Mutex::new(HeaderMap::new())),
        status,
        location,
        delay,
    };
    async fn handler(State(state): State<HttpState>, headers: HeaderMap) -> impl IntoResponse {
        state.calls.fetch_add(1, Ordering::SeqCst);
        *state.seen.lock().unwrap() = headers;
        tokio::time::sleep(state.delay).await;
        let mut headers = HeaderMap::new();
        if let Some(location) = state.location {
            headers.insert(header::LOCATION, location.parse().unwrap());
        }
        (state.status, headers, "PRIVATE-UPSTREAM-RESPONSE")
    }
    let router = Router::new()
        .fallback(any(handler))
        .with_state(state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    HttpFixture { url, state, task }
}
async fn profile(
    app: &TestApp,
    token: &str,
    code: &str,
    configuration: Value,
    secret: Value,
) -> Value {
    let(status,value)=app.send("POST","/api/v1/settings/profiles",json!({"name":code,"code":code,"enabled":true,"configuration":configuration,"secret":secret}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
async fn probe(app: &TestApp, token: &str, row: &Value) -> (StatusCode, Value) {
    app.send(
        "POST",
        &format!(
            "/api/v1/settings/profiles/{}/probe",
            row["id"].as_str().unwrap()
        ),
        json!({"version":row["version"]}),
        Some(token),
    )
    .await
}

#[tokio::test]
async fn http_probe_uses_saved_credentials_and_persists_safe_versioned_results() {
    let server = fixture(StatusCode::OK, None, Duration::ZERO).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "http-probe",
        json!({"kind":"http","baseUrl":server.url}),
        json!({"kind":"http","headerName":"X-Probe-Key","value":"PRIVATE-PROBE-KEY"}),
    )
    .await;
    let (status, result) = probe(&app, token, &row).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["data"]["success"], true);
    assert_eq!(result["data"]["scope"], "httpHead");
    assert_eq!(result["data"]["isCurrent"], true);
    assert_eq!(result["data"]["upstreamStatus"], 200);
    {
        let headers = server.state.seen.lock().unwrap();
        assert_eq!(headers["x-probe-key"], "PRIVATE-PROBE-KEY");
        assert!(!headers.contains_key("authorization"));
    }
    let (_, current) = app
        .send(
            "GET",
            &format!("/api/v1/settings/profiles/{}", row["id"].as_str().unwrap()),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(current["data"]["lastCheck"]["id"], result["data"]["id"]);
    let (status, history) = app
        .send(
            "GET",
            &format!(
                "/api/v1/settings/profiles/{}/checks",
                row["id"].as_str().unwrap()
            ),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["meta"]["total"], 1);
    assert!(!history.to_string().contains("PRIVATE-PROBE-KEY"));
    assert!(!history.to_string().contains("PRIVATE-UPSTREAM-RESPONSE"));
    app.cleanup().await;
}
#[tokio::test]
async fn redirects_are_not_followed_and_unreachable_or_metadata_targets_are_safe_failures() {
    let target = fixture(StatusCode::OK, None, Duration::ZERO).await;
    let redirect = fixture(
        StatusCode::TEMPORARY_REDIRECT,
        Some(target.url.clone()),
        Duration::ZERO,
    )
    .await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "redirect",
        json!({"kind":"http","baseUrl":redirect.url}),
        Value::Null,
    )
    .await;
    let (status, result) = probe(&app, token, &row).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["data"]["success"], false);
    assert_eq!(result["data"]["errorCode"], "REDIRECT_REJECTED");
    assert_eq!(target.state.calls.load(Ordering::SeqCst), 0);
    let row = profile(
        &app,
        token,
        "metadata",
        json!({"kind":"http","baseUrl":"https://169.254.169.254/"}),
        Value::Null,
    )
    .await;
    let (status, result) = probe(&app, token, &row).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["data"]["errorCode"], "DESTINATION_BLOCKED");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let row = profile(
        &app,
        token,
        "unavailable",
        json!({"kind":"http","baseUrl":format!("http://{address}")}),
        Value::Null,
    )
    .await;
    assert_eq!(probe(&app, token, &row).await.1["data"]["success"], false);
    app.cleanup().await;
}
#[tokio::test]
async fn changing_a_profile_while_checking_does_not_publish_an_old_result_as_current() {
    let server = fixture(StatusCode::OK, None, Duration::from_millis(300)).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "changing",
        json!({"kind":"http","baseUrl":server.url}),
        Value::Null,
    )
    .await;
    let check = probe(&app, token, &row);
    let update = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.state.calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("probe did not reach fixture");
        app.send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{}", row["id"].as_str().unwrap()),
            json!({"version":1,"name":"Changed"}),
            Some(token),
        )
        .await
    };
    let (result, changed) = tokio::join!(check, update);
    assert_eq!(changed.0, StatusCode::OK);
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.1["data"]["isCurrent"], false);
    let (_, current) = app
        .send(
            "GET",
            &format!("/api/v1/settings/profiles/{}", row["id"].as_str().unwrap()),
            Value::Null,
            Some(token),
        )
        .await;
    assert!(current["data"]["lastCheck"].is_null());
    app.cleanup().await;
}
#[tokio::test]
async fn mongodb_check_uses_the_real_replica_set_and_bad_auth_does_not_leak_credentials() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let configuration = json!({"kind":"mongodb","hosts":[{"host":"127.0.0.1","port":13317}],"database":"admin","authDatabase":"admin","replicaSet":"tri-admin-rs","tls":false,"directConnection":true});
    let row = profile(
        &app,
        token,
        "mongo-check",
        configuration.clone(),
        Value::Null,
    )
    .await;
    let (status, result) = probe(&app, token, &row).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["data"]["success"], true);
    assert_eq!(result["data"]["scope"], "mongodbPing");
    let row=profile(&app,token,"mongo-auth",configuration,json!({"kind":"mongodb","username":"missing-fixture-user","password":"PRIVATE-MONGO-PASSWORD"})).await;
    let (status, result) = probe(&app, token, &row).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["data"]["success"], false);
    assert!(!result.to_string().contains("PRIVATE-MONGO-PASSWORD"));
    app.cleanup().await;
}
#[tokio::test]
async fn expected_versions_and_permissions_are_checked_before_network_requests() {
    let server = fixture(StatusCode::OK, None, Duration::ZERO).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "version-check",
        json!({"kind":"http","baseUrl":server.url}),
        Value::Null,
    )
    .await;
    assert_eq!(
        app.send(
            "POST",
            &format!(
                "/api/v1/settings/profiles/{}/probe",
                row["id"].as_str().unwrap()
            ),
            json!({"version":2}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        probe(&app, "invalid", &row).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 0);
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"settings"}, doc! {"$set":{"version":1}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}

fn configured(app: &TestApp, root: &std::path::Path, ca: Option<&std::path::Path>) -> TestApp {
    let config = admin_api::config::Config::from_lookup(|name| match name {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_STORAGE_ROOT" => Some(root.to_string_lossy().into()),
        "TRI_OUTBOUND_CA_FILE" => ca.map(|path| path.to_string_lossy().into()),
        _ => None,
    })
    .unwrap();
    TestApp {
        router: admin_api::build_router(admin_api::state::AppState::new(
            config,
            Some(app.database.clone()),
        )),
        database: app.database.clone(),
    }
}
#[tokio::test]
async fn local_storage_check_writes_reads_and_removes_only_its_temp_file() {
    let directory = tempfile::Builder::new()
        .prefix("tri-admin-connection-")
        .tempdir()
        .unwrap();
    assert!(directory.path().starts_with(std::env::temp_dir()));
    let root = directory.path().join("storage");
    let app = TestApp::new().await;
    let configured = configured(&app, &root, None);
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "local-check",
        json!({"kind":"localStorage","subdirectory":"managed/nested"}),
        Value::Null,
    )
    .await;
    let (status, result) = probe(&configured, token, &row).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["data"]["success"], true);
    assert_eq!(result["data"]["scope"], "localWriteReadDelete");
    assert_eq!(
        std::fs::read_dir(root.join("managed/nested"))
            .unwrap()
            .count(),
        0
    );
    let restarted = configured_router(&app, &root);
    let (_, view) = restarted
        .send(
            "GET",
            &format!("/api/v1/settings/profiles/{}", row["id"].as_str().unwrap()),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(view["data"]["lastCheck"]["isCurrent"], false);
    app.cleanup().await;
}
fn configured_router(app: &TestApp, root: &std::path::Path) -> TestApp {
    configured(app, root, None)
}

#[tokio::test]
async fn ipv6_loopback_http_profiles_are_valid_without_loosening_destination_policy() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "ipv6-loopback",
        json!({"kind":"http","baseUrl":"http://[::1]:9900"}),
        Value::Null,
    )
    .await;
    assert_eq!(row["configuration"]["baseUrl"], "http://[::1]:9900");
    app.cleanup().await;
}

struct TlsFixture {
    url: String,
    ip_url: String,
    pem: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for TlsFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn tls_fixture() -> TlsFixture {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let key = rcgen::KeyPair::generate().unwrap();
    let certificate = rcgen::CertificateParams::new(vec!["localhost".to_owned()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let pem = certificate.pem();
    let private = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()),
    );
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.der().clone()], private)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(stream).await else {
                    return;
                };
                let mut bytes = vec![];
                let mut byte = [0u8; 1];
                while bytes.len() < 16384 {
                    if stream.read_exact(&mut byte).await.is_err() {
                        return;
                    }
                    bytes.push(byte[0]);
                    if bytes.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                let _ = stream.shutdown().await;
            });
        }
    });
    TlsFixture {
        url: format!("https://localhost:{port}"),
        ip_url: format!("https://127.0.0.1:{port}"),
        pem,
        task,
    }
}
#[tokio::test]
async fn tls_checks_require_trust_and_continue_to_validate_the_original_hostname() {
    let app = TestApp::new().await;
    let server = tls_fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "tls-check",
        json!({"kind":"http","baseUrl":server.url}),
        Value::Null,
    )
    .await;
    let result = probe(&app, token, &row).await;
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.1["data"]["success"], false);
    let directory = tempfile::Builder::new()
        .prefix("tri-admin-connection-")
        .tempdir()
        .unwrap();
    assert!(directory.path().starts_with(std::env::temp_dir()));
    let ca = directory.path().join("ca.pem");
    std::fs::write(&ca, &server.pem).unwrap();
    let trusted = configured(&app, directory.path(), Some(&ca));
    let result = probe(&trusted, token, &row).await;
    assert_eq!(result.0, StatusCode::OK, "{:?}", result.1);
    assert_eq!(result.1["data"]["success"], true, "{:?}", result.1);
    let wrong = profile(
        &app,
        token,
        "wrong-host",
        json!({"kind":"http","baseUrl":server.ip_url}),
        Value::Null,
    )
    .await;
    assert_eq!(
        probe(&trusted, token, &wrong).await.1["data"]["success"],
        false
    );
    app.cleanup().await;
}

struct SmtpFixture {
    host: String,
    port: u16,
    auth: Arc<AtomicUsize>,
    mail: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for SmtpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn smtp_fixture() -> SmtpFixture {
    use base64::Engine;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let auth = Arc::new(AtomicUsize::new(0));
    let mail = Arc::new(AtomicUsize::new(0));
    let a = auth.clone();
    let m = mail.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let auth = a.clone();
            let mail = m.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut read = BufReader::new(read);
                let _ = write.write_all(b"220 fixture ESMTP\r\n").await;
                let mut stage = 0u8;
                let mut user = false;
                loop {
                    let mut line = String::new();
                    if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    let upper = line.to_ascii_uppercase();
                    let response = if stage > 0 {
                        let value = base64::engine::general_purpose::STANDARD
                            .decode(line)
                            .unwrap_or_default();
                        if stage == 1 {
                            stage = 0;
                            auth.fetch_add(1, Ordering::SeqCst);
                            if value == b"\0fixture-user\0PRIVATE-SMTP-PASSWORD" {
                                b"235 authenticated\r\n".as_slice()
                            } else {
                                b"535 PRIVATE-SMTP-FAILURE\r\n".as_slice()
                            }
                        } else if stage == 2 {
                            user = value == b"fixture-user";
                            stage = 3;
                            b"334 UGFzc3dvcmQ6\r\n".as_slice()
                        } else {
                            stage = 0;
                            auth.fetch_add(1, Ordering::SeqCst);
                            if user && value == b"PRIVATE-SMTP-PASSWORD" {
                                b"235 authenticated\r\n".as_slice()
                            } else {
                                b"535 PRIVATE-SMTP-FAILURE\r\n".as_slice()
                            }
                        }
                    } else if upper.starts_with("EHLO") {
                        b"250-fixture\r\n250-AUTH PLAIN LOGIN\r\n250 OK\r\n".as_slice()
                    } else if upper.starts_with("AUTH PLAIN ") {
                        auth.fetch_add(1, Ordering::SeqCst);
                        if base64::engine::general_purpose::STANDARD
                            .decode(&line[11..])
                            .unwrap_or_default()
                            == b"\0fixture-user\0PRIVATE-SMTP-PASSWORD"
                        {
                            b"235 authenticated\r\n".as_slice()
                        } else {
                            b"535 PRIVATE-SMTP-FAILURE\r\n".as_slice()
                        }
                    } else if upper == "AUTH PLAIN" {
                        stage = 1;
                        b"334 \r\n".as_slice()
                    } else if upper == "AUTH LOGIN" {
                        stage = 2;
                        b"334 VXNlcm5hbWU6\r\n".as_slice()
                    } else if upper == "NOOP" {
                        b"250 OK\r\n".as_slice()
                    } else if upper == "QUIT" {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        break;
                    } else if upper.starts_with("MAIL")
                        || upper.starts_with("RCPT")
                        || upper == "DATA"
                    {
                        mail.fetch_add(1, Ordering::SeqCst);
                        b"550 no mail allowed\r\n".as_slice()
                    } else {
                        b"500 unsupported\r\n".as_slice()
                    };
                    if write.write_all(response).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    SmtpFixture {
        host: "127.0.0.1".into(),
        port,
        auth,
        mail,
        task,
    }
}
#[tokio::test]
async fn smtp_probes_authenticate_without_sending_mail_and_do_not_downgrade_starttls() {
    let server = smtp_fixture().await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let configuration = json!({"kind":"smtp","host":server.host,"port":server.port,"security":"plain","fromAddress":"ops@example.test"});
    let row = profile(
        &app,
        token,
        "smtp-check",
        configuration.clone(),
        json!({"kind":"smtp","username":"fixture-user","password":"PRIVATE-SMTP-PASSWORD"}),
    )
    .await;
    let result = probe(&app, token, &row).await;
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.1["data"]["success"], true, "{:?}", result.1);
    assert_eq!(server.auth.load(Ordering::SeqCst), 1);
    assert_eq!(server.mail.load(Ordering::SeqCst), 0);
    let wrong = profile(
        &app,
        token,
        "smtp-wrong",
        configuration.clone(),
        json!({"kind":"smtp","username":"fixture-user","password":"WRONG-PRIVATE-PASSWORD"}),
    )
    .await;
    let rejected = probe(&app, token, &wrong).await;
    assert_eq!(rejected.0, StatusCode::OK);
    assert_eq!(rejected.1["data"]["errorCode"], "SMTP_AUTH_REJECTED");
    assert!(!rejected.1.to_string().contains("PRIVATE-SMTP-FAILURE"));
    let mut tls = configuration;
    tls["security"] = json!("startTls");
    let row = profile(
        &app,
        token,
        "smtp-tls",
        tls,
        json!({"kind":"smtp","username":"fixture-user","password":"PRIVATE-SMTP-PASSWORD"}),
    )
    .await;
    assert_eq!(probe(&app, token, &row).await.1["data"]["success"], false);
    assert_eq!(server.auth.load(Ordering::SeqCst), 2);
    assert_eq!(server.mail.load(Ordering::SeqCst), 0);
    app.cleanup().await;
}

#[tokio::test]
async fn local_directory_links_cannot_escape_the_configured_root() {
    let directory = tempfile::Builder::new()
        .prefix("tri-admin-connection-")
        .tempdir()
        .unwrap();
    assert!(directory.path().starts_with(std::env::temp_dir()));
    let root = directory.path().join("root");
    let outside = directory.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"keep").unwrap();
    let link = root.join("linked");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "New-Item -ItemType Junction -Path '{}' -Target '{}' | Out-Null",
            link.to_string_lossy().replace('\'', "''"),
            outside.to_string_lossy().replace('\'', "''")
        );
        let result = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(result.status.success());
    }
    let app = TestApp::new().await;
    let checked = configured(&app, &root, None);
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "escaped-check",
        json!({"kind":"localStorage","subdirectory":"linked"}),
        Value::Null,
    )
    .await;
    let (status, result) = probe(&checked, token, &row).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["data"]["success"], false);
    assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn capacity_is_bounded_and_http_checks_time_out_without_holding_configuration_locks() {
    let server = fixture(StatusCode::OK, None, Duration::from_secs(20)).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "slow-check",
        json!({"kind":"http","baseUrl":server.url}),
        Value::Null,
    )
    .await;
    let start = std::time::Instant::now();
    let batch = futures_util::future::join_all((0..8).map(|_| probe(&app, token, &row)));
    let ninth = async {
        tokio::time::timeout(Duration::from_secs(3), async {
            while server.state.calls.load(Ordering::SeqCst) < 8 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        probe(&app, token, &row).await
    };
    let (results, limited) = tokio::join!(batch, ninth);
    assert_eq!(limited.0, StatusCode::TOO_MANY_REQUESTS);
    for result in results {
        assert_eq!(result.0, StatusCode::OK);
        assert_eq!(result.1["data"]["errorCode"], "CHECK_TIMEOUT");
    }
    assert!(start.elapsed() < Duration::from_secs(9));
    app.cleanup().await;
}

#[tokio::test]
async fn result_persistence_failure_rolls_back_last_check_and_audit() {
    let server = fixture(StatusCode::OK, None, Duration::ZERO).await;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = profile(
        &app,
        token,
        "rollback-check",
        json!({"kind":"http","baseUrl":server.url}),
        Value::Null,
    )
    .await;
    app.database.run_command(doc!{"collMod":"configuration_checks","validator":{"$jsonSchema":{"required":["rejectFixture"]}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        probe(&app, token, &row).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("configuration_checks")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"settings.profiles.probe"})
            .await
            .unwrap(),
        0
    );
    let (_, current) = app
        .send(
            "GET",
            &format!("/api/v1/settings/profiles/{}", row["id"].as_str().unwrap()),
            Value::Null,
            Some(token),
        )
        .await;
    assert!(current["data"]["lastCheck"].is_null());
    app.cleanup().await;
}
