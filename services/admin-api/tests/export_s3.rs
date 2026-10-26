#[path = "support/s3_faults.rs"]
mod s3_faults;
mod support;
use admin_api::{
    build_router,
    config::Config,
    modules::{
        data_tools::export_jobs::{maintenance, worker},
        files::storage::{BlobId, BlobSpec, BlobStore, StorageError},
    },
    state::AppState,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use s3_faults::{Fault, S3FaultProxy};
use serde_json::{Value, json};
use support::{KEY, TestApp};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

async fn fixture(
    fault: Fault,
) -> (
    TestApp,
    AppState,
    tempfile::TempDir,
    S3FaultProxy,
    String,
    ObjectId,
) {
    let definition: Value = serde_json::from_slice(
        &std::fs::read(
            std::env::var("TRI_S3_FIXTURE_FILE")
                .expect("run node tools/test-s3-content.mjs --exports-only"),
        )
        .unwrap(),
    )
    .unwrap();
    let proxy = S3FaultProxy::start(&definition, fault).await;
    let mut app = TestApp::new().await;
    let root = tempfile::tempdir().unwrap();
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_STORAGE_ROOT" => Some(root.path().to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    let state = AppState::new(config, Some(app.database.clone()));
    app.router = build_router(state.clone());
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap().to_owned();
    let (code, profile) = app.send("POST", "/api/v1/settings/profiles", json!({"name":"export-s3","code":"export-s3","enabled":true,"configuration":{"kind":"s3","endpoint":proxy.endpoint,"bucket":definition["bucket"],"region":definition["region"],"prefix":format!("exports-{}",uuid::Uuid::new_v4()),"pathStyle":true},"secret":{"kind":"s3","accessKeyId":definition["accessKeyId"],"secretAccessKey":definition["secretAccessKey"]}}), Some(&token)).await;
    assert_eq!(code, StatusCode::CREATED);
    let profile = ObjectId::parse_str(profile["data"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"S3 fixture","key":"s3.export","valueType":"boolean","value":true}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    (app, state, root, proxy, token, profile)
}
async fn enqueue(app: &TestApp, token: &str, profile: ObjectId) -> ObjectId {
    let (code, result) = app.send("POST", "/api/v1/data-tools/exports", json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":"csv","maxRows":10,"storageProfileId":profile.to_hex(),"source":{"kind":"resource","resource":"parameters","columns":[{"field":"key","header":"key"},{"field":"value","header":"value"}],"search":"s3.export"}}), Some(token)).await;
    assert_eq!(code, StatusCode::ACCEPTED, "{result}");
    ObjectId::parse_str(result["data"]["id"].as_str().unwrap()).unwrap()
}
async fn status(app: &TestApp, token: &str, id: ObjectId) -> Value {
    app.send(
        "GET",
        &format!("/api/v1/data-tools/exports/{id}"),
        Value::Null,
        Some(token),
    )
    .await
    .1["data"]
        .clone()
}
async fn attempt(app: &TestApp, id: ObjectId) -> Document {
    app.database
        .collection::<Document>("data_export_attempts")
        .find_one(doc! {"jobId":id})
        .sort(doc! {"generation":-1})
        .await
        .unwrap()
        .unwrap()
}
async fn due(app: &TestApp, id: ObjectId) {
    for collection in ["data_exports", "data_export_attempts"] {
        let filter = if collection == "data_exports" {
            doc! {"_id":id}
        } else {
            doc! {"jobId":id}
        };
        app.database
            .collection::<Document>(collection)
            .update_many(filter, doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}})
            .await
            .unwrap();
    }
}
async fn content(app: &TestApp, token: &str, id: ObjectId) -> (StatusCode, Vec<u8>) {
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/data-tools/exports/{id}/content"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    if code == StatusCode::OK {
        assert_eq!(response.headers()["cache-control"], "private, no-store");
    }
    (
        code,
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}
async fn assert_missing(app: &TestApp, state: &AppState, candidate: &Document) {
    let store = BlobStore::from_profile(
        &app.database,
        state.config.clone(),
        candidate.get_object_id("storageProfileId").unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        store
            .read(
                BlobId::parse(&candidate.get_object_id("blobId").unwrap().to_hex()).unwrap(),
                BlobSpec::new(
                    candidate.get_i64("size").unwrap() as u64,
                    candidate.get_str("sha256").unwrap()
                )
                .unwrap(),
                CancellationToken::new()
            )
            .await,
        Err(StorageError::NotFound)
    ));
}

#[tokio::test]
#[ignore = "requires isolated real S3; run npm run test:s3"]
async fn real_s3_lost_ack_reconciles_original_bytes_once_and_expiry_removes_the_object() {
    let (app, state, _root, proxy, token, profile) = fixture(Fault::DropAcknowledgement).await;
    let id = enqueue(&app, &token, profile).await;
    assert!(worker::run_once(&state).await.unwrap());
    assert_eq!(status(&app, &token, id).await["state"], "unknown");
    let original = attempt(&app, id).await;
    assert_eq!(content(&app, &token, id).await.0, StatusCode::CONFLICT);
    app.database
        .collection::<Document>("parameters")
        .update_one(
            doc! {"key":"s3.export"},
            doc! {"$set":{"valueJson":"false"}},
        )
        .await
        .unwrap();
    proxy.set_fault(Fault::CorruptRead);
    due(&app, id).await;
    assert!(worker::run_once(&state).await.unwrap());
    assert_eq!(status(&app, &token, id).await["state"], "unknown");
    assert_eq!(content(&app, &token, id).await.0, StatusCode::CONFLICT);
    assert_eq!(proxy.puts(), 1);
    proxy.set_fault(Fault::None);
    due(&app, id).await;
    assert!(worker::run_once(&state).await.unwrap());
    assert_eq!(status(&app, &token, id).await["state"], "completed");
    let (_, bytes) = content(&app, &token, id).await;
    assert_eq!(
        String::from_utf8(bytes)
            .unwrap()
            .trim_start_matches('\u{feff}'),
        "key,value\ns3.export,true\n"
    );
    assert_eq!(proxy.puts(), 1);
    assert_eq!(
        attempt(&app, id).await.get_object_id("blobId").unwrap(),
        original.get_object_id("blobId").unwrap()
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.complete","targetId":id})
            .await
            .unwrap(),
        1
    );
    app.database
        .collection::<Document>("data_exports")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    due(&app, id).await;
    assert_eq!(maintenance::run_once(&state).await.unwrap().retired, 1);
    assert_missing(&app, &state, &original).await;
    app.cleanup().await;
}

#[tokio::test]
#[ignore = "requires isolated real S3; run npm run test:s3"]
async fn real_s3_late_write_after_cancel_keeps_reference_until_confirmed_and_deleted() {
    let (app, state, _root, proxy, token, profile) = fixture(Fault::LateWrite).await;
    let id = enqueue(&app, &token, profile).await;
    worker::run_once(&state).await.unwrap();
    let original = attempt(&app, id).await;
    assert_eq!(status(&app, &token, id).await["state"], "unknown");
    due(&app, id).await;
    worker::run_once(&state).await.unwrap();
    assert_eq!(status(&app, &token, id).await["state"], "unknown");
    assert_eq!(proxy.puts(), 1);
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/exports/{id}/cancel"),
            json!({}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::OK
    );
    due(&app, id).await;
    assert_eq!(maintenance::run_once(&state).await.unwrap().retired, 0);
    let raw = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert!(raw.get_bool("storageRetained").unwrap());
    assert_eq!(status(&app, &token, id).await["cleanupPending"], true);
    proxy.release_late_write().await;
    due(&app, id).await;
    assert_eq!(maintenance::run_once(&state).await.unwrap().retired, 1);
    assert_eq!(status(&app, &token, id).await["state"], "cancelled");
    assert_eq!(status(&app, &token, id).await["cleanupPending"], false);
    assert_eq!(content(&app, &token, id).await.0, StatusCode::CONFLICT);
    assert_missing(&app, &state, &original).await;
    assert_eq!(proxy.puts(), 1);
    app.cleanup().await;
}

#[tokio::test]
#[ignore = "requires isolated real S3; run npm run test:s3"]
async fn real_s3_explicit_write_rejection_is_failed_and_retry_does_not_wait_for_a_nonexistent_object()
 {
    let (app, state, _root, proxy, token, profile) = fixture(Fault::RejectWrite).await;
    let id = enqueue(&app, &token, profile).await;
    worker::run_once(&state).await.unwrap();
    let rejected = status(&app, &token, id).await;
    assert_eq!(rejected["state"], "failed");
    assert_eq!(rejected["cleanupPending"], false);
    assert!(!rejected.to_string().contains("private-storage-denial"));
    let original = attempt(&app, id).await;
    assert!(!original.get_bool("writeStarted").unwrap());
    assert_eq!(original.get_str("state").unwrap(), "cleaned");
    assert_eq!(proxy.reads(), 0);
    proxy.set_fault(Fault::None);
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/exports/{id}/retry"),
            json!({"requestId":uuid::Uuid::new_v4().to_string()}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    worker::run_once(&state).await.unwrap();
    assert_eq!(status(&app, &token, id).await["state"], "completed");
    assert_ne!(
        attempt(&app, id).await.get_object_id("blobId").unwrap(),
        original.get_object_id("blobId").unwrap()
    );
    assert_eq!(proxy.puts(), 2);
    app.cleanup().await;
}

#[tokio::test]
#[ignore = "requires isolated real S3; run npm run test:s3"]
async fn real_s3_existing_candidate_with_denied_read_is_unknown_until_original_can_be_verified() {
    let (app, state, _root, proxy, token, profile) = fixture(Fault::ExistingWithDeniedRead).await;
    let id = enqueue(&app, &token, profile).await;
    worker::run_once(&state).await.unwrap();
    let original = attempt(&app, id).await;
    assert_eq!(status(&app, &token, id).await["state"], "unknown");
    assert!(original.get_bool("writeStarted").unwrap());
    app.send(
        "POST",
        &format!("/api/v1/data-tools/exports/{id}/retry"),
        json!({"requestId":uuid::Uuid::new_v4().to_string()}),
        Some(&token),
    )
    .await;
    worker::run_once(&state).await.unwrap();
    assert_eq!(status(&app, &token, id).await["state"], "unknown");
    assert_eq!(proxy.puts(), 1);
    assert_eq!(
        app.database
            .collection::<Document>("data_export_attempts")
            .count_documents(doc! {"jobId":id})
            .await
            .unwrap(),
        1
    );
    proxy.set_fault(Fault::None);
    due(&app, id).await;
    worker::run_once(&state).await.unwrap();
    assert_eq!(status(&app, &token, id).await["state"], "completed");
    assert_eq!(content(&app, &token, id).await.0, StatusCode::OK);
    assert_eq!(proxy.puts(), 1);
    assert_eq!(
        attempt(&app, id).await.get_object_id("blobId").unwrap(),
        original.get_object_id("blobId").unwrap()
    );
    app.cleanup().await;
}

#[tokio::test]
#[ignore = "requires isolated real S3; run npm run test:s3"]
async fn real_s3_write_rejection_after_cancellation_releases_candidate_without_overwriting_cancelled_state()
 {
    let (app, state, _root, proxy, token, profile) = fixture(Fault::RejectAfterRelease).await;
    let id = enqueue(&app, &token, profile).await;
    let running = tokio::spawn({
        let state = state.clone();
        async move { worker::run_once(&state).await }
    });
    proxy.wait_for_write().await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/exports/{id}/cancel"),
            json!({}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::OK
    );
    proxy.release();
    assert!(running.await.unwrap().unwrap());
    let cancelled = status(&app, &token, id).await;
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["cleanupPending"], false);
    let candidate = attempt(&app, id).await;
    assert_eq!(candidate.get_str("state").unwrap(), "cleaned");
    assert!(!candidate.get_bool("writeStarted").unwrap());
    assert_eq!(proxy.puts(), 1);
    assert_eq!(proxy.reads(), 0);
    assert_eq!(maintenance::run_once(&state).await.unwrap().retired, 0);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.complete","targetId":id})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}
