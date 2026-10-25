mod support;
use admin_api::{
    build_router,
    config::Config,
    modules::files::storage::{BlobId, BlobSpec, BlobStore, VerifiedSource},
    state::AppState,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
fn config(root: &std::path::Path) -> Config {
    Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_STORAGE_ROOT" => Some(root.to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap()
}
async fn app() -> (support::TestApp, tempfile::TempDir) {
    let app = support::TestApp::new().await;
    let root = tempfile::tempdir().unwrap();
    (
        support::TestApp {
            router: build_router(AppState::new(
                config(root.path()),
                Some(app.database.clone()),
            )),
            database: app.database,
        },
        root,
    )
}
async fn upload(app: &support::TestApp, token: &str) -> Value {
    let(status,row)=app.send("POST","/api/v1/uploads",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"recovery.bin","mediaType":"application/octet-stream","size":4,"sha256":format!("{:x}",Sha256::digest(b"data"))}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED);
    row["data"].clone()
}
async fn candidate(app: &support::TestApp, upload: &Value) -> ObjectId {
    let id = ObjectId::new();
    app.database.collection::<Document>("file_attempts").insert_one(doc!{"uploadId":ObjectId::parse_str(upload["id"].as_str().unwrap()).unwrap(),"ownerId":ObjectId::parse_str(upload["ownerId"].as_str().unwrap()).unwrap(),"storageProfileId":ObjectId::parse_str(upload["storageProfileId"].as_str().unwrap()).unwrap(),"blobId":id,"kind":"chunk","index":0i64,"size":4i64,"sha256":format!("{:x}",Sha256::digest(b"data")),"state":"writing","leaseUntil":DateTime::from_millis(0),"createdAt":DateTime::now(),"updatedAt":DateTime::now()}).await.unwrap();
    app.database
        .collection::<Document>("file_uploads")
        .update_one(
            doc! {"_id":ObjectId::parse_str(upload["id"].as_str().unwrap()).unwrap()},
            doc! {"$inc":{"attemptCount":1i64}},
        )
        .await
        .unwrap();
    id
}
async fn physical(root: &std::path::Path, id: ObjectId) {
    let config = Arc::new(config(root));
    let spec = BlobSpec::new(4, &format!("{:x}", Sha256::digest(b"data"))).unwrap();
    let source = VerifiedSource::stage(&config, b"data".as_slice(), spec, CancellationToken::new())
        .await
        .unwrap();
    BlobStore::local(config, ".tri-upload-chunks")
        .await
        .unwrap()
        .put(
            BlobId::parse(&id.to_hex()).unwrap(),
            source,
            CancellationToken::new(),
        )
        .await
        .unwrap();
}
async fn put(app: &support::TestApp, token: &str, id: &str) -> StatusCode {
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/uploads/{id}/chunks/0"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/octet-stream")
        .header("x-chunk-sha256", format!("{:x}", Sha256::digest(b"data")))
        .body(Body::from(b"data".to_vec()))
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let _ = to_bytes(response.into_body(), 65536).await.unwrap();
    status
}
#[tokio::test]
async fn a_verified_expired_chunk_is_adopted_once_without_retransmission() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = upload(&app, token).await;
    let blob = candidate(&app, &row).await;
    physical(root.path(), blob).await;
    let id = row["id"].as_str().unwrap();
    let (status, recovered) = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/recover"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(recovered["data"]["recovered"], 1);
    let status = app
        .send(
            "GET",
            &format!("/api/v1/uploads/{id}"),
            Value::Null,
            Some(token),
        )
        .await
        .1;
    assert_eq!(status["data"]["receivedIndices"], json!([0]));
    assert_eq!(status["data"]["receivedBytes"], 4);
    assert_eq!(put(&app, token, id).await, StatusCode::OK);
    assert_eq!(
        app.database
            .collection::<Document>("file_chunks")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn absent_expired_io_stays_unknown_and_a_late_object_does_not_replace_the_new_receipt() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = upload(&app, token).await;
    let old = candidate(&app, &row).await;
    let id = row["id"].as_str().unwrap();
    let (status, recovered) = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/recover"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered["data"]["unknown"], 1);
    assert_eq!(put(&app, token, id).await, StatusCode::OK);
    let accepted = app
        .database
        .collection::<Document>("file_chunks")
        .find_one(doc! {"uploadId":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("blobId")
        .unwrap();
    assert_ne!(accepted, old);
    physical(root.path(), old).await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/recover"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        !root
            .path()
            .join(".tri-upload-chunks/objects")
            .join(old.to_hex())
            .exists()
    );
    assert_eq!(
        std::fs::read(
            root.path()
                .join(".tri-upload-chunks/objects")
                .join(accepted.to_hex())
        )
        .unwrap(),
        b"data"
    );
    let status = app
        .send(
            "GET",
            &format!("/api/v1/uploads/{id}"),
            Value::Null,
            Some(token),
        )
        .await
        .1;
    assert_eq!(status["data"]["receivedCount"], 1);
    app.cleanup().await;
}
#[tokio::test]
async fn expired_writers_do_not_consume_all_receiving_capacity() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = upload(&app, token).await;
    for _ in 0..8 {
        candidate(&app, &row).await;
    }
    assert_eq!(
        put(&app, token, row["id"].as_str().unwrap()).await,
        StatusCode::OK
    );
    app.cleanup().await;
}
