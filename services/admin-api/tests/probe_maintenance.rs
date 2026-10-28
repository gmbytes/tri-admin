#[path = "support/storage.rs"]
mod storage_support;
mod support;
use admin_api::{build_router, config::Config, modules::files::jobs, state::AppState};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::json;
use sha2::Digest;
use std::{collections::HashMap, sync::Arc};
async fn fixture() -> (
    support::TestApp,
    AppState,
    tempfile::TempDir,
    Arc<tokio::sync::Mutex<HashMap<String, Vec<u8>>>>,
    tokio::task::JoinHandle<()>,
    String,
) {
    let app = support::TestApp::new().await;
    let root = tempfile::tempdir().unwrap();
    let objects = Arc::new(tokio::sync::Mutex::new(HashMap::<String, Vec<u8>>::new()));
    let store = objects.clone();
    let router = axum::Router::new().fallback(move |request: Request<Body>| {
        let store = store.clone();
        async move {
            let path = request.uri().path().to_owned();
            match request.method().clone() {
                Method::GET => match store.lock().await.get(&path) {
                    Some(data) => Response::new(Body::from(data.clone())),
                    None => StatusCode::NOT_FOUND.into_response(),
                },
                Method::DELETE => {
                    store.lock().await.remove(&path);
                    StatusCode::NO_CONTENT.into_response()
                }
                _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_STORAGE_ROOT" => Some(root.path().to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    let state = AppState::new(config, Some(app.database.clone()));
    let app = support::TestApp {
        router: build_router(state.clone()),
        database: app.database,
    };
    (app, state, root, objects, server, endpoint)
}
async fn profile(app: &support::TestApp, _token: &str, endpoint: &str) -> ObjectId {
    let row=storage_support::seed(app,json!({"name":"probe-store","code":"probe-store","enabled":true,"configuration":{"kind":"s3","endpoint":endpoint,"bucket":"test-bucket","region":"test","prefix":"probe-test","pathStyle":true},"secret":{"kind":"s3","accessKeyId":"fixture","secretAccessKey":"fixture-private-secret"}})).await;
    ObjectId::parse_str(row["id"].as_str().unwrap()).unwrap()
}
async fn journal(app: &support::TestApp, profile: ObjectId, phase: i64) -> String {
    let nonce = uuid::Uuid::new_v4().to_string();
    app.database.collection::<Document>("file_storage_probes").insert_one(doc!{"_id":&nonce,"storageProfileId":profile,"phase":phase,"state":"unknown","leaseUntil":DateTime::from_millis(0),"nextRunAt":DateTime::from_millis(0),"createdAt":DateTime::now(),"updatedAt":DateTime::now()}).await.unwrap();
    nonce
}
async fn due(app: &support::TestApp, id: &str) {
    app.database
        .collection::<Document>("file_storage_probes")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
}
#[tokio::test]
async fn a_late_first_probe_write_is_tracked_until_cleaned() {
    let (app, state, _root, objects, server, endpoint) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, &endpoint).await;
    let nonce = journal(&app, profile, 1).await;
    jobs::maintenance_once(&state).await.unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("file_storage_probes")
            .find_one(doc! {"_id":&nonce})
            .await
            .unwrap()
            .unwrap()
            .get_str("state")
            .unwrap(),
        "unknown"
    );
    let key = format!("/test-bucket/probe-test/_tri-probes/{nonce}");
    objects.lock().await.insert(key.clone(), vec![0]);
    due(&app, &nonce).await;
    jobs::maintenance_once(&state).await.unwrap();
    assert!(!objects.lock().await.contains_key(&key));
    assert_eq!(
        app.database
            .collection::<Document>("file_storage_probes")
            .find_one(doc! {"_id":&nonce})
            .await
            .unwrap()
            .unwrap()
            .get_str("state")
            .unwrap(),
        "done"
    );
    server.abort();
    app.cleanup().await;
}
#[tokio::test]
async fn an_unknown_second_put_is_not_cleaned_while_only_the_first_value_is_visible() {
    let (app, state, _root, objects, server, endpoint) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, &endpoint).await;
    let nonce = journal(&app, profile, 2).await;
    let key = format!("/test-bucket/probe-test/_tri-probes/{nonce}");
    objects.lock().await.insert(key.clone(), vec![0]);
    jobs::maintenance_once(&state).await.unwrap();
    assert_eq!(objects.lock().await.get(&key), Some(&vec![0]));
    assert_eq!(
        app.database
            .collection::<Document>("file_storage_probes")
            .find_one(doc! {"_id":&nonce})
            .await
            .unwrap()
            .unwrap()
            .get_str("state")
            .unwrap(),
        "unknown"
    );
    objects.lock().await.insert(key.clone(), vec![1]);
    due(&app, &nonce).await;
    jobs::maintenance_once(&state).await.unwrap();
    assert!(!objects.lock().await.contains_key(&key));
    server.abort();
    app.cleanup().await;
}

#[tokio::test]
async fn a_probe_fenced_by_maintenance_cannot_issue_its_second_put() {
    use admin_api::modules::files::storage::{
        BlobId, BlobSpec, BlobStore, StorageError, VerifiedSource,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;
    let app = support::TestApp::new().await;
    let root = tempfile::tempdir().unwrap();
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_STORAGE_ROOT" => Some(root.path().to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    let state = AppState::new(config, Some(app.database.clone()));
    let app = support::TestApp {
        router: build_router(state.clone()),
        database: app.database,
    };
    let objects = Arc::new(tokio::sync::Mutex::new(HashMap::<String, Vec<u8>>::new()));
    let seen = CancellationToken::new();
    let release = CancellationToken::new();
    let puts = Arc::new(AtomicUsize::new(0));
    let context = (objects, seen.clone(), release.clone(), puts.clone());
    let router = axum::Router::new().fallback(move |request: Request<Body>| {
        let (objects, seen, release, puts) = context.clone();
        async move {
            let path = request.uri().path().to_owned();
            match request.method().clone() {
                Method::PUT => {
                    puts.fetch_add(1, Ordering::SeqCst);
                    let bytes = axum::body::to_bytes(request.into_body(), 16).await.unwrap();
                    objects.lock().await.insert(path, bytes.to_vec());
                    seen.cancel();
                    release.cancelled().await;
                    StatusCode::OK.into_response()
                }
                Method::GET => match objects.lock().await.get(&path) {
                    Some(data) => Response::new(Body::from(data.clone())),
                    None => StatusCode::NOT_FOUND.into_response(),
                },
                Method::DELETE => {
                    objects.lock().await.remove(&path);
                    StatusCode::NO_CONTENT.into_response()
                }
                _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, &endpoint).await;
    let store = BlobStore::from_profile(&app.database, state.config.clone(), profile)
        .await
        .unwrap();
    let source = VerifiedSource::stage(
        &state.config,
        b"data".as_slice(),
        BlobSpec::new(4, &format!("{:x}", sha2::Sha256::digest(b"data"))).unwrap(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let writer = tokio::spawn(async move {
        store
            .put(BlobId::new(), source, CancellationToken::new())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), seen.cancelled())
        .await
        .unwrap();
    app.database.collection::<Document>("file_storage_probes").update_many(doc!{"storageProfileId":profile},doc!{"$set":{"leaseUntil":DateTime::from_millis(0),"nextRunAt":DateTime::from_millis(0)}}).await.unwrap();
    jobs::maintenance_once(&state).await.unwrap();
    release.cancel();
    assert_eq!(writer.await.unwrap().unwrap_err(), StorageError::Cancelled);
    assert_eq!(puts.load(Ordering::SeqCst), 1);
    server.abort();
    app.cleanup().await;
}
