#[path = "support/storage.rs"]
mod storage_support;
mod support;
use admin_api::{
    config::Config,
    modules::files::storage::{
        BlobId, BlobSpec, BlobStore, PutResult, StorageError, VerifiedSource,
    },
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
fn config(root: &std::path::Path) -> Arc<Config> {
    Arc::new(
        Config::from_lookup(|key| match key {
            "TRI_JWT_SIGNING_KEY" => Some("fixture-signing-key-at-least-32-bytes-long".into()),
            "TRI_STORAGE_ROOT" => Some(root.to_string_lossy().into_owned()),
            "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
            "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
            _ => None,
        })
        .unwrap(),
    )
}
fn spec(data: &[u8]) -> BlobSpec {
    BlobSpec::new(data.len() as u64, &format!("{:x}", Sha256::digest(data))).unwrap()
}
async fn source(config: &Config, data: &[u8]) -> VerifiedSource {
    VerifiedSource::stage(config, data, spec(data), CancellationToken::new())
        .await
        .unwrap()
}
async fn bytes(store: &BlobStore, id: BlobId, spec: BlobSpec) -> Result<Vec<u8>, StorageError> {
    let mut stream = store.read(id, spec, CancellationToken::new()).await?;
    let mut data = Vec::new();
    while let Some(item) = stream.next().await {
        data.extend(item?);
    }
    Ok(data)
}
#[tokio::test]
async fn local_content_is_immutable_verified_and_deleted_by_exact_id() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let store = BlobStore::local(config.clone(), "managed").await.unwrap();
    let id = BlobId::new();
    let payload = b"verified local content";
    assert_eq!(
        store
            .put(id, source(&config, payload).await, CancellationToken::new())
            .await
            .unwrap(),
        PutResult::Created
    );
    assert_eq!(
        store
            .put(id, source(&config, payload).await, CancellationToken::new())
            .await
            .unwrap(),
        PutResult::AlreadyPresent
    );
    assert_eq!(
        store
            .put(
                id,
                source(&config, b"different").await,
                CancellationToken::new()
            )
            .await
            .unwrap_err(),
        StorageError::Conflict
    );
    assert_eq!(bytes(&store, id, spec(payload)).await.unwrap(), payload);
    assert_eq!(
        bytes(&store, id, spec(b"wrong-length")).await.unwrap_err(),
        StorageError::IntegrityMismatch
    );
    std::fs::write(
        root.path().join("managed").join("unrelated.txt"),
        b"preserve",
    )
    .unwrap();
    store.delete(id, CancellationToken::new()).await.unwrap();
    store.delete(id, CancellationToken::new()).await.unwrap();
    assert_eq!(
        bytes(&store, id, spec(payload)).await.unwrap_err(),
        StorageError::NotFound
    );
    assert_eq!(
        std::fs::read(root.path().join("managed/unrelated.txt")).unwrap(),
        b"preserve"
    );
    assert_eq!(
        std::fs::read_dir(root.path().join(".tri-staging"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn staging_rejects_wrong_length_digest_and_cancellation_without_publishing() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    for (payload, expected) in [
        (b"bad".as_slice(), spec(b"good")),
        (b"same-length".as_slice(), spec(b"same-lengtx")),
    ] {
        assert!(matches!(
            VerifiedSource::stage(&config, payload, expected, CancellationToken::new()).await,
            Err(StorageError::IntegrityMismatch)
        ));
    }
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        VerifiedSource::stage(&config, b"data".as_slice(), spec(b"data"), cancelled).await,
        Err(StorageError::Cancelled)
    ));
    assert!(BlobSpec::new(5 * 1024 * 1024 * 1024 + 1, &"0".repeat(64)).is_err());
    assert!(BlobSpec::new(1, "../secret").is_err());
    assert!(BlobId::parse("../../outside").is_err());
    assert_eq!(
        std::fs::read_dir(root.path().join(".tri-staging"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn concurrent_local_publication_has_one_winner_and_does_not_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let store = Arc::new(BlobStore::local(config.clone(), "managed").await.unwrap());
    let id = BlobId::new();
    let a = source(&config, b"alpha").await;
    let b = source(&config, b"bravo").await;
    let (a, b) = tokio::join!(
        store.put(id, a, CancellationToken::new()),
        store.put(id, b, CancellationToken::new())
    );
    assert!(matches!(
        (&a, &b),
        (Ok(PutResult::Created), Err(StorageError::Conflict))
            | (Err(StorageError::Conflict), Ok(PutResult::Created))
    ));
    let expected = if a.is_ok() { b"alpha" } else { b"bravo" };
    assert_eq!(bytes(&store, id, spec(expected)).await.unwrap(), expected);
}
#[tokio::test]
async fn local_download_detects_same_length_tampering_and_can_be_cancelled() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let store = BlobStore::local(config.clone(), "managed").await.unwrap();
    let id = BlobId::new();
    store
        .put(
            id,
            source(&config, b"original").await,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    std::fs::write(
        root.path().join("managed/objects").join(id.to_string()),
        b"tampered",
    )
    .unwrap();
    assert_eq!(
        bytes(&store, id, spec(b"original")).await.unwrap_err(),
        StorageError::IntegrityMismatch
    );
    let token = CancellationToken::new();
    let mut stream = store
        .read(id, spec(b"original"), token.clone())
        .await
        .unwrap();
    token.cancel();
    assert_eq!(
        stream.next().await.unwrap().unwrap_err(),
        StorageError::Cancelled
    );
}
#[tokio::test]
#[ignore = "requires the isolated Docker fixture; run npm run test:s3"]
async fn real_s3_content_writes_reads_retries_rejects_overwrites_and_deletes() {
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("TRI_S3_FIXTURE_FILE").expect("run npm run test:s3")).unwrap(),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let app = support::TestApp::new().await;
    let mut ids = Vec::new();
    for (code, key) in [
        ("file-good", fixture["secretAccessKey"].as_str().unwrap()),
        ("file-bad", "wrong-secret"),
    ] {
        let row=storage_support::seed(&app,serde_json::json!({"name":code,"code":code,"enabled":true,"configuration":{"kind":"s3","endpoint":fixture["endpoint"],"bucket":fixture["bucket"],"region":fixture["region"],"prefix":"file-acceptance","pathStyle":true},"secret":{"kind":"s3","accessKeyId":fixture["accessKeyId"],"secretAccessKey":key}})).await;
        ids.push(mongodb::bson::oid::ObjectId::parse_str(row["id"].as_str().unwrap()).unwrap());
    }
    let store = BlobStore::from_profile(&app.database, config.clone(), ids[0])
        .await
        .unwrap();
    let id = BlobId::new();
    let payload = vec![b'x'; 12 * 1024 * 1024];
    assert_eq!(
        store
            .put(
                id,
                source(&config, &payload).await,
                CancellationToken::new()
            )
            .await
            .unwrap(),
        PutResult::Created
    );
    assert_eq!(bytes(&store, id, spec(&payload)).await.unwrap(), payload);
    assert_eq!(
        store
            .put(
                id,
                source(&config, &payload).await,
                CancellationToken::new()
            )
            .await
            .unwrap(),
        PutResult::AlreadyPresent
    );
    assert_eq!(
        store
            .put(
                id,
                source(&config, b"do not overwrite").await,
                CancellationToken::new()
            )
            .await
            .unwrap_err(),
        StorageError::Conflict
    );
    let wrong = BlobStore::from_profile(&app.database, config.clone(), ids[1])
        .await
        .unwrap();
    assert_eq!(
        bytes(&wrong, id, spec(&payload)).await.unwrap_err(),
        StorageError::AuthenticationRejected
    );
    store.delete(id, CancellationToken::new()).await.unwrap();
    store.delete(id, CancellationToken::new()).await.unwrap();
    assert_eq!(
        bytes(&store, id, spec(&payload)).await.unwrap_err(),
        StorageError::NotFound
    );
    let race = BlobId::new();
    let first = source(&config, b"alpha").await;
    let second = source(&config, b"bravo").await;
    let (a, b) = tokio::join!(
        store.put(race, first, CancellationToken::new()),
        store.put(race, second, CancellationToken::new())
    );
    assert!(
        matches!(
            (&a, &b),
            (Ok(PutResult::Created), Err(StorageError::Conflict))
                | (Err(StorageError::Conflict), Ok(PutResult::Created))
        ),
        "unexpected race results: {a:?} / {b:?}"
    );
    let winner = if a.is_ok() { b"alpha" } else { b"bravo" };
    assert_eq!(bytes(&store, race, spec(winner)).await.unwrap(), winner);
    store.delete(race, CancellationToken::new()).await.unwrap();
    app.cleanup().await;
}

#[tokio::test]
#[ignore = "requires the isolated Garage fixture; run npm run test:s3"]
async fn real_s3_without_atomic_create_is_rejected_before_user_content() {
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("TRI_S3_FIXTURE_FILE").expect("run npm run test:s3")).unwrap(),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let app = support::TestApp::new().await;
    let row=storage_support::seed(&app,serde_json::json!({"name":"unsupported","code":"unsupported","enabled":true,"configuration":{"kind":"s3","endpoint":fixture["endpoint"],"bucket":fixture["bucket"],"region":fixture["region"],"prefix":"refused-objects","pathStyle":true},"secret":{"kind":"s3","accessKeyId":fixture["accessKeyId"],"secretAccessKey":fixture["secretAccessKey"]}})).await;
    let profile = mongodb::bson::oid::ObjectId::parse_str(row["id"].as_str().unwrap()).unwrap();
    let store = BlobStore::from_profile(&app.database, config.clone(), profile)
        .await
        .unwrap();
    let id = BlobId::new();
    assert_eq!(
        store
            .put(
                id,
                source(&config, b"never publish this").await,
                CancellationToken::new()
            )
            .await
            .unwrap_err(),
        StorageError::Unsupported
    );
    assert_eq!(
        bytes(&store, id, spec(b"never publish this"))
            .await
            .unwrap_err(),
        StorageError::NotFound
    );
    app.cleanup().await;
}

#[tokio::test]
async fn a_large_generated_source_streams_through_staging_and_download() {
    use tokio::io::AsyncReadExt;
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let size = 32 * 1024 * 1024u64;
    let mut hash = Sha256::new();
    for _ in 0..(size / (64 * 1024)) {
        hash.update([b'z'; 64 * 1024]);
    }
    let expected = BlobSpec::new(size, &format!("{:x}", hash.finalize())).unwrap();
    let source = VerifiedSource::stage(
        &config,
        tokio::io::repeat(b'z').take(size),
        expected.clone(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let store = BlobStore::local(config, "managed").await.unwrap();
    let id = BlobId::new();
    store
        .put(id, source, CancellationToken::new())
        .await
        .unwrap();
    let mut stream = store
        .read(id, expected, CancellationToken::new())
        .await
        .unwrap();
    let mut count = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        assert!(chunk.len() <= 64 * 1024);
        count += chunk.len() as u64;
    }
    assert_eq!(count, size);
}

#[tokio::test]
async fn cancellation_wakes_an_idle_source_and_leaves_no_staged_content() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let token = CancellationToken::new();
    let worker_token = token.clone();
    let (reader, _writer) = tokio::io::duplex(1);
    let worker = tokio::spawn(async move {
        VerifiedSource::stage(&config, reader, spec(b"data"), worker_token).await
    });
    tokio::task::yield_now().await;
    token.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(StorageError::Cancelled)));
    assert_eq!(
        std::fs::read_dir(root.path().join(".tri-staging"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn cancelling_a_sent_remote_write_is_unknown_and_the_stored_content_can_be_queried() {
    use axum::{
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode},
        response::{IntoResponse, Response},
    };
    use std::collections::HashMap;
    let objects = Arc::new(tokio::sync::Mutex::new(HashMap::<String, Vec<u8>>::new()));
    let committed = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let state = (objects.clone(), committed.clone(), release.clone());
    let router = axum::Router::new().fallback(move |request: Request<Body>| {
        let (objects, committed, release) = state.clone();
        async move {
            let path = request.uri().path().to_owned();
            let method = request.method().clone();
            if method == Method::PUT {
                if request
                    .headers()
                    .get("if-none-match")
                    .and_then(|value| value.to_str().ok())
                    != Some("*")
                {
                    return StatusCode::BAD_REQUEST.into_response();
                }
                let bytes = to_bytes(request.into_body(), 1024 * 1024).await.unwrap();
                let mut rows = objects.lock().await;
                if rows.contains_key(&path) {
                    return StatusCode::PRECONDITION_FAILED.into_response();
                }
                rows.insert(path.clone(), bytes.to_vec());
                drop(rows);
                if path.contains("/objects/") {
                    committed.notify_one();
                    release.notified().await;
                }
                StatusCode::OK.into_response()
            } else if method == Method::GET {
                match objects.lock().await.get(&path) {
                    Some(value) => Response::new(Body::from(value.clone())),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            } else if method == Method::DELETE {
                objects.lock().await.remove(&path);
                StatusCode::NO_CONTENT.into_response()
            } else {
                StatusCode::METHOD_NOT_ALLOWED.into_response()
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let app = support::TestApp::new().await;
    let row=storage_support::seed(&app,serde_json::json!({"name":"remote","code":"remote","enabled":true,"configuration":{"kind":"s3","endpoint":endpoint,"bucket":"test-bucket","region":"test","prefix":"","pathStyle":true},"secret":{"kind":"s3","accessKeyId":"fixture","secretAccessKey":"fixture-private-key"}})).await;
    let profile = mongodb::bson::oid::ObjectId::parse_str(row["id"].as_str().unwrap()).unwrap();
    let store = Arc::new(
        BlobStore::from_profile(&app.database, config.clone(), profile)
            .await
            .unwrap(),
    );
    let id = BlobId::new();
    let input = source(&config, b"data").await;
    let cancel = CancellationToken::new();
    let worker_token = cancel.clone();
    let worker_store = store.clone();
    let worker = tokio::spawn(async move { worker_store.put(id, input, worker_token).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), committed.notified())
        .await
        .unwrap();
    cancel.cancel();
    assert_eq!(
        worker.await.unwrap().unwrap_err(),
        StorageError::OutcomeUnknown
    );
    assert_eq!(bytes(&store, id, spec(b"data")).await.unwrap(), b"data");
    release.notify_waiters();
    store.delete(id, CancellationToken::new()).await.unwrap();
    server.abort();
    let _ = server.await;
    app.cleanup().await;
}

#[tokio::test]
async fn storage_profile_consumers_reject_disabled_wrong_kind_and_missing_keys() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let app = support::TestApp::new().await;
    let _signed = app.login().await;
    for input in [
        serde_json::json!({"name":"disabled","code":"disabled","enabled":false,"configuration":{"kind":"localStorage","subdirectory":"disabled"}}),
        serde_json::json!({"name":"http","code":"http","enabled":true,"configuration":{"kind":"http","baseUrl":"https://example.test"}}),
    ] {
        let row = storage_support::seed(&app, input).await;
        let id = mongodb::bson::oid::ObjectId::parse_str(row["id"].as_str().unwrap()).unwrap();
        assert!(matches!(
            BlobStore::from_profile(&app.database, config.clone(), id).await,
            Err(StorageError::ConfigurationUnavailable)
        ));
    }
    let row=storage_support::seed(&app,serde_json::json!({"name":"s3","code":"s3","enabled":true,"configuration":{"kind":"s3","endpoint":"https://objects.example.test","bucket":"test-bucket","region":"us-east-1","prefix":"","pathStyle":true},"secret":{"kind":"s3","accessKeyId":"fixture","secretAccessKey":"private-fixture-key"}})).await;
    let id = mongodb::bson::oid::ObjectId::parse_str(row["id"].as_str().unwrap()).unwrap();
    let without_keys = Arc::new(
        Config::from_lookup(|key| (key == "TRI_JWT_SIGNING_KEY").then(|| support::KEY.into()))
            .unwrap(),
    );
    assert!(matches!(
        BlobStore::from_profile(&app.database, without_keys, id).await,
        Err(StorageError::SecretUnavailable)
    ));
    app.cleanup().await;
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn local_storage_refuses_directory_links_that_escape_the_storage_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("sentinel"), b"preserve").unwrap();
    let link = root.path().join("escape");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    #[cfg(windows)]
    {
        assert!(
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(outside.path())
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    assert!(
        BlobStore::local(config(root.path()), "escape")
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(outside.path().join("sentinel")).unwrap(),
        b"preserve"
    );
    assert!(!outside.path().join("objects").exists());
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&link).unwrap();
}
