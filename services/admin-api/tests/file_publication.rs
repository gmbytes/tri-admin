mod support;
use admin_api::{build_router, config::Config, state::AppState};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::TestApp;
use tower::ServiceExt;
async fn app() -> (TestApp, tempfile::TempDir) {
    let app = TestApp::new().await;
    let root = tempfile::tempdir().unwrap();
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_STORAGE_ROOT" => Some(root.path().to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    (
        TestApp {
            router: build_router(AppState::new(config, Some(app.database.clone()))),
            database: app.database,
        },
        root,
    )
}
async fn upload(
    app: &TestApp,
    token: &str,
    name: &str,
    data: &[u8],
    expected: Option<&str>,
) -> String {
    let hash = expected
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{:x}", Sha256::digest(data)));
    let(status,result)=app.send("POST","/api/v1/uploads",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":name,"mediaType":"application/octet-stream","size":data.len(),"sha256":hash}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    let id = result["data"]["id"].as_str().unwrap();
    for (index, chunk) in data.chunks(4 * 1024 * 1024).enumerate() {
        put_chunk(app, token, id, index, chunk).await;
    }
    id.into()
}
async fn put_chunk(app: &TestApp, token: &str, id: &str, index: usize, data: &[u8]) {
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/uploads/{id}/chunks/{index}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/octet-stream")
        .header("x-chunk-sha256", format!("{:x}", Sha256::digest(data)))
        .body(Body::from(data.to_vec()))
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&to_bytes(response.into_body(), 65536).await.unwrap())
    );
}
async fn wait(app: &TestApp, token: &str, id: &str, states: &[&str]) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            let (status, row) = app
                .send(
                    "GET",
                    &format!("/api/v1/uploads/{id}"),
                    Value::Null,
                    Some(token),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            if states.contains(&row["data"]["state"].as_str().unwrap()) {
                return row["data"].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("publication did not reach the expected durable state")
}
#[tokio::test]
async fn completed_upload_publishes_once_and_downloads_authenticated_verified_bytes() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let data = b"published contents";
    let id = upload(&app, token, "资料 文件.bin", data, None).await;
    let first = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(first.0, StatusCode::ACCEPTED, "{}", first.1);
    let completed = wait(&app, token, &id, &["completed"]).await;
    let file = completed["fileId"].as_str().unwrap();
    let (status, metadata) = app
        .send(
            "GET",
            &format!("/api/v1/files/{file}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(metadata["data"]["name"], "资料 文件.bin");
    assert!(!metadata.to_string().contains("blobId"));
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/files/{file}/content"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("filename*=UTF-8''")
    );
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        data.as_slice()
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/files/{file}/content"),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"files.publish"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/files/{file}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/files/{file}"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/retry"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/files/{file}"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.cleanup().await;
}
#[tokio::test]
async fn empty_files_publish_but_missing_chunks_and_wrong_whole_digest_do_not() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = upload(&app, token, "empty.bin", b"", None).await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    wait(&app, token, &id, &["completed"]).await;
    let id = upload(
        &app,
        token,
        "bad-digest.bin",
        b"data",
        Some(&"0".repeat(64)),
    )
    .await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let failed = wait(&app, token, &id, &["failed"]).await;
    assert_eq!(failed["lastError"], "CHECKSUM_MISMATCH");
    assert!(failed["fileId"].is_null());
    let(status,row)=app.send("POST","/api/v1/uploads",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"missing.bin","mediaType":"application/octet-stream","size":4,"sha256":format!("{:x}",Sha256::digest(b"data"))}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = row["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    app.cleanup().await;
}
#[tokio::test]
async fn a_missing_chunk_is_invalidated_and_repair_requires_the_original_digest() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = upload(&app, token, "repair.bin", b"data", None).await;
    let chunk = app
        .database
        .collection::<Document>("file_chunks")
        .find_one(doc! {"uploadId":ObjectId::parse_str(&id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(
        root.path()
            .join(".tri-upload-chunks/objects")
            .join(chunk.get_object_id("blobId").unwrap().to_hex()),
    )
    .unwrap();
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    let row = wait(&app, token, &id, &["receiving"]).await;
    assert_eq!(row["lastError"], "CHUNK_MISSING");
    assert_eq!(row["receivedCount"], 0);
    assert_eq!(row["receivedIndices"], json!([]));
    let wrong = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/uploads/{id}/chunks/0"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/octet-stream")
        .header("x-chunk-sha256", format!("{:x}", Sha256::digest(b"xxxx")))
        .body(Body::from(b"xxxx".to_vec()))
        .unwrap();
    assert_eq!(
        app.router.clone().oneshot(wrong).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    put_chunk(&app, token, &id, 0, b"data").await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    wait(&app, token, &id, &["completed"]).await;
    app.cleanup().await;
}
#[tokio::test]
async fn file_edits_are_versioned_and_deleted_content_stays_unavailable() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = upload(&app, token, "edit.bin", b"data", None).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    let done = wait(&app, token, &id, &["completed"]).await;
    let file = done["fileId"].as_str().unwrap();
    let (status, row) = app
        .send(
            "PATCH",
            &format!("/api/v1/files/{file}"),
            json!({"version":1,"name":"renamed.bin"}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(row["data"]["version"], 2);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/files/{file}"),
            json!({"version":1,"name":"stale.bin"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/files/{file}?version=2"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/files/{file}/content"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.cleanup().await;
}

#[tokio::test]
async fn publication_metadata_failure_remains_queryable_and_reconciles_without_another_object() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = upload(&app, token, "recover.bin", b"recovery contents", None).await;
    app.database
        .run_command(doc! {"collMod":"files","validator":{"rejectFixture":{"$exists":true}}})
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let pending = wait(&app, token, &id, &["unknown"]).await;
    assert_eq!(pending["lastError"], "PERSISTENCE_PENDING");
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"files.publish"})
            .await
            .unwrap(),
        0
    );
    app.database
        .run_command(doc! {"collMod":"files","validator":{}})
        .await
        .unwrap();
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    wait(&app, token, &id, &["completed"]).await;
    assert_eq!(
        app.database
            .collection::<Document>("file_attempts")
            .count_documents(doc! {"kind":"publish"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("managed/objects"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"files.publish"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

async fn role(
    app: &TestApp,
    owner: &str,
    slug: &str,
    scope: &str,
    scope_roles: Vec<String>,
) -> String {
    let(status,row)=app.send("POST","/api/v1/roles",json!({"name":slug,"slug":slug,"permissions":["files.read","files.write"],"dataScope":scope,"dataScopeRoleIds":scope_roles}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{row}");
    row["data"]["id"].as_str().unwrap().into()
}
async fn user(app: &TestApp, owner: &str, name: &str, role: &str) -> (String, String) {
    let(status,row)=app.send("POST","/api/v1/users",json!({"username":name,"displayName":name,"password":support::PASSWORD,"roleIds":[role]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{row}");
    let id = row["data"]["id"].as_str().unwrap().to_owned();
    let (_, auth) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":support::PASSWORD}),
            None,
        )
        .await;
    (id, auth["data"]["accessToken"].as_str().unwrap().into())
}
#[tokio::test]
async fn published_file_scope_uses_current_owner_roles_before_pagination_and_every_operation() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let a = role(&app, owner, "team-a", "self", vec![]).await;
    let b = role(&app, owner, "team-b", "self", vec![]).await;
    let selected = role(&app, owner, "selected", "roles", vec![a.clone()]).await;
    let (bob_id, bob) = user(&app, owner, "file-bob", &a).await;
    let (_, viewer) = user(&app, owner, "file-viewer", &selected).await;
    let first = upload(&app, &bob, "a-bob.bin", b"bob", None).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{first}/complete"),
        json!({}),
        Some(&bob),
    )
    .await;
    let first = wait(&app, &bob, &first, &["completed"]).await["fileId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{first}/retry"),
            json!({}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let second = upload(&app, &viewer, "z-viewer.bin", b"viewer", None).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{second}/complete"),
        json!({}),
        Some(&viewer),
    )
    .await;
    wait(&app, &viewer, &second, &["completed"]).await;
    let (status, own) = app
        .send(
            "GET",
            "/api/v1/files?pageSize=1&sort=name",
            Value::Null,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(own["meta"]["total"], 1);
    assert_eq!(own["data"][0]["id"], first);
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/files/{second}"),
            Value::Null,
            Some(&bob)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/files/{second}?version=1"),
            Value::Null,
            Some(&bob)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (status, rows) = app
        .send(
            "GET",
            "/api/v1/files?pageSize=1&sort=name",
            Value::Null,
            Some(&viewer),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    assert_eq!(rows["meta"]["total"], 2);
    assert_eq!(rows["data"][0]["id"], first);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{bob_id}"),
            json!({"version":1,"roleIds":[b]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, rows) = app
        .send(
            "GET",
            "/api/v1/files?pageSize=1&sort=name",
            Value::Null,
            Some(&viewer),
        )
        .await;
    assert_eq!(rows["meta"]["total"], 1);
    assert_eq!(rows["data"][0]["id"], second);
    for path in [
        format!("/api/v1/files/{first}"),
        format!("/api/v1/files/{first}/content"),
    ] {
        assert_eq!(
            app.send("GET", &path, Value::Null, Some(&viewer)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/files/{first}"),
            json!({"version":1,"name":"denied.bin"}),
            Some(&viewer)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/files/{first}?version=1"),
            Value::Null,
            Some(&viewer)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/files/{first}"),
            Value::Null,
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_completion_has_one_candidate_and_corrupted_downloads_never_finish() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = upload(&app, token, "concurrent.bin", b"original", None).await;
    let path = format!("/api/v1/uploads/{id}/complete");
    let (a, b) = tokio::join!(
        app.send("POST", &path, json!({}), Some(token)),
        app.send("POST", &path, json!({}), Some(token))
    );
    assert!(matches!(a.0, StatusCode::ACCEPTED | StatusCode::OK));
    assert!(matches!(b.0, StatusCode::ACCEPTED | StatusCode::OK));
    let done = wait(&app, token, &id, &["completed"]).await;
    let file = done["fileId"].as_str().unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("file_attempts")
            .count_documents(doc! {"kind":"publish"})
            .await
            .unwrap(),
        1
    );
    let row = app
        .database
        .collection::<Document>("files")
        .find_one(doc! {"_id":ObjectId::parse_str(file).unwrap()})
        .await
        .unwrap()
        .unwrap();
    std::fs::write(
        root.path()
            .join("managed/objects")
            .join(row.get_object_id("blobId").unwrap().to_hex()),
        b"tampered",
    )
    .unwrap();
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/files/{file}/content"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(to_bytes(response.into_body(), 1024).await.is_err());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if app
                .database
                .collection::<Document>("request_errors")
                .count_documents(doc! {"errorCode":"FILE_CONTENT_INTEGRITY"})
                .await
                .unwrap()
                == 1
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("late download integrity failure was not persisted");
    let error = app
        .database
        .collection::<Document>("request_errors")
        .find_one(doc! {"errorCode":"FILE_CONTENT_INTEGRITY"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        error.get_str("route").unwrap(),
        "/api/v1/files/{id}/content"
    );
    assert!(!format!("{error:?}").contains("tampered"));
    app.cleanup().await;
}

struct MockS3 {
    endpoint: String,
    received: tokio_util::sync::CancellationToken,
    release: tokio_util::sync::CancellationToken,
    committed: tokio_util::sync::CancellationToken,
    writes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}
async fn corrupt_unknown_candidate(
    app: &TestApp,
    root: &std::path::Path,
    id: &str,
    contents: &[u8],
) -> ObjectId {
    let upload_id = ObjectId::parse_str(id).unwrap();
    let parent = app
        .database
        .collection::<Document>("file_uploads")
        .find_one(doc! {"_id":upload_id})
        .await
        .unwrap()
        .unwrap();
    let candidate = ObjectId::new();
    let blob = ObjectId::new();
    let now = mongodb::bson::DateTime::now();
    app.database.collection::<Document>("file_attempts").insert_one(doc! {
        "_id":candidate,"uploadId":upload_id,"ownerId":parent.get_object_id("ownerId").unwrap(),
        "storageProfileId":parent.get_object_id("storageProfileId").unwrap(),"kind":"publish","blobId":blob,
        "size":parent.get_i64("size").unwrap(),"sha256":parent.get_str("sha256").unwrap(),
        "state":"unknown","retryRequested":false,"leaseToken":null,"leaseUntil":now,"nextRunAt":now,"createdAt":now,"updatedAt":now
    }).await.unwrap();
    app.database.collection::<Document>("file_uploads").update_one(doc! {"_id":upload_id},doc! {"$set":{"state":"unknown","publishAttemptId":candidate},"$inc":{"attemptCount":1i64}}).await.unwrap();
    let directory = root.join("managed/objects");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join(blob.to_hex()), contents).unwrap();
    candidate
}

#[tokio::test]
async fn explicit_retry_retires_corrupt_candidates_without_reusing_their_keys() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    for corrupted in [b"x".as_slice(), b"invalid bytes!".as_slice()] {
        let id = upload(&app, token, "corrupt-retry.bin", b"valid contents", None).await;
        let old = corrupt_unknown_candidate(&app, root.path(), &id, corrupted).await;
        assert_eq!(
            app.send(
                "POST",
                &format!("/api/v1/uploads/{id}/retry"),
                json!({}),
                Some(token)
            )
            .await
            .0,
            StatusCode::ACCEPTED
        );
        wait(&app, token, &id, &["completed"]).await;
        let retired = app
            .database
            .collection::<Document>("file_attempts")
            .find_one(doc! {"_id":old})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retired.get_str("state").unwrap(), "cleaned");
        let current = app
            .database
            .collection::<Document>("files")
            .find_one(doc! {"uploadId":ObjectId::parse_str(&id).unwrap()})
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            retired.get_object_id("blobId").unwrap(),
            current.get_object_id("blobId").unwrap()
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn cancelled_corrupt_candidate_can_be_cleaned_without_publishing_it() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = upload(&app, token, "corrupt-cancel.bin", b"valid contents", None).await;
    let old = corrupt_unknown_candidate(&app, root.path(), &id, b"invalid bytes!").await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/cancel"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/cleanup"),
        json!({}),
        Some(token),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let row = app
                .database
                .collection::<Document>("file_attempts")
                .find_one(doc! {"_id":old})
                .await
                .unwrap()
                .unwrap();
            if row.get_str("state").unwrap() == "cleaned" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("cancelled corrupt candidate was not cleaned");
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}
impl Drop for MockS3 {
    fn drop(&mut self) {
        self.release.cancel();
        self.server.abort();
    }
}
async fn mock_s3(deferred: bool, error_response: bool) -> MockS3 {
    use axum::{
        http::Method,
        response::{IntoResponse, Response},
    };
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio_util::sync::CancellationToken;
    let objects = Arc::new(tokio::sync::Mutex::new(HashMap::<String, Vec<u8>>::new()));
    let received = CancellationToken::new();
    let release = CancellationToken::new();
    let committed = CancellationToken::new();
    let writes = Arc::new(AtomicUsize::new(0));
    let state = (
        objects,
        received.clone(),
        release.clone(),
        committed.clone(),
        writes.clone(),
    );
    let router = axum::Router::new().fallback(move |request: Request<Body>| {
        let (objects, received, release, committed, writes) = state.clone();
        async move {
            let path = request.uri().path().to_owned();
            match request.method().clone() {
                Method::PUT => {
                    if request
                        .headers()
                        .get("if-none-match")
                        .and_then(|value| value.to_str().ok())
                        != Some("*")
                    {
                        return StatusCode::BAD_REQUEST.into_response();
                    }
                    let bytes = to_bytes(request.into_body(), 1024 * 1024)
                        .await
                        .unwrap()
                        .to_vec();
                    let mut map = objects.lock().await;
                    if map.contains_key(&path) {
                        return StatusCode::PRECONDITION_FAILED.into_response();
                    }
                    if path.contains("/_tri-probes/") {
                        map.insert(path, bytes);
                        return StatusCode::OK.into_response();
                    }
                    writes.fetch_add(1, Ordering::SeqCst);
                    if deferred {
                        drop(map);
                        let store = objects.clone();
                        let key = path.clone();
                        let gate = release.clone();
                        let finished = committed.clone();
                        tokio::spawn(async move {
                            gate.cancelled().await;
                            store.lock().await.insert(key, bytes);
                            finished.cancel();
                        });
                    } else {
                        map.insert(path, bytes);
                        drop(map);
                        committed.cancel();
                    }
                    received.cancel();
                    if error_response {
                        (StatusCode::INTERNAL_SERVER_ERROR, "private-upstream-error")
                            .into_response()
                    } else {
                        release.cancelled().await;
                        StatusCode::OK.into_response()
                    }
                }
                Method::GET => match objects.lock().await.get(&path) {
                    Some(value) => Response::new(Body::from(value.clone())),
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
    MockS3 {
        endpoint,
        received,
        release,
        committed,
        writes,
        server,
    }
}
async fn default_s3(app: &TestApp, owner: &str, endpoint: &str) {
    let(status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"fixture-s3","code":"fixture-s3","enabled":true,"configuration":{"kind":"s3","endpoint":endpoint,"bucket":"test-bucket","region":"test","prefix":"publication","pathStyle":true},"secret":{"kind":"s3","accessKeyId":"fixture","secretAccessKey":"private-fixture-secret"}}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    let (_, settings) = app
        .send("GET", "/api/v1/settings/service", Value::Null, Some(owner))
        .await;
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":settings["data"]["version"],"storageProfileId":profile["data"]["id"]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
}
#[tokio::test]
async fn a_remote_error_after_storage_commit_reconciles_by_reading_without_another_put() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let server = mock_s3(false, true).await;
    default_s3(&app, token, &server.endpoint).await;
    let id = upload(&app, token, "remote-error.bin", b"remote contents", None).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    let unknown = wait(&app, token, &id, &["unknown"]).await;
    assert!(!unknown.to_string().contains("private-upstream-error"));
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    wait(&app, token, &id, &["completed"]).await;
    assert_eq!(server.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn explicit_retry_checks_the_existing_object_before_creating_another_candidate() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let server = mock_s3(false, true).await;
    default_s3(&app, token, &server.endpoint).await;
    let id = upload(
        &app,
        token,
        "explicit-existing.bin",
        b"existing retry",
        None,
    )
    .await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    wait(&app, token, &id, &["unknown"]).await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/retry"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    wait(&app, token, &id, &["completed"]).await;
    assert_eq!(server.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        app.database
            .collection::<Document>("file_attempts")
            .count_documents(doc! {"kind":"publish"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn explicit_retry_uses_a_fresh_key_and_retains_the_late_old_write_for_cleanup() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let server = mock_s3(true, true).await;
    default_s3(&app, token, &server.endpoint).await;
    let id = upload(&app, token, "explicit-missing.bin", b"missing retry", None).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    wait(&app, token, &id, &["unknown"]).await;
    let upload_id = ObjectId::parse_str(&id).unwrap();
    let old = app
        .database
        .collection::<Document>("file_attempts")
        .find_one(doc! {"uploadId":upload_id,"kind":"publish"})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("_id")
        .unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/retry"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while server.writes.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("explicit retry did not create the fresh candidate");
    assert_eq!(
        app.database
            .collection::<Document>("file_attempts")
            .find_one(doc! {"_id":old})
            .await
            .unwrap()
            .unwrap()
            .get_str("state")
            .unwrap(),
        "orphanUnknown"
    );
    server.release.cancel();
    server.committed.cancelled().await;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    wait(&app, token, &id, &["completed"]).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/cleanup"),
        json!({}),
        Some(token),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            if app
                .database
                .collection::<Document>("file_attempts")
                .find_one(doc! {"_id":old})
                .await
                .unwrap()
                .unwrap()
                .get_str("state")
                .unwrap()
                == "cleaned"
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("late old write was not retained and cleaned");
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"files.publish"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn cancelling_unknown_remote_io_stays_pending_until_the_late_object_is_observed() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let server = mock_s3(true, false).await;
    default_s3(&app, token, &server.endpoint).await;
    let id = upload(&app, token, "remote-cancel.bin", b"late contents", None).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        server.received.cancelled(),
    )
    .await
    .unwrap();
    let cancel = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cancel"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(cancel.0, StatusCode::OK);
    let row = wait(&app, token, &id, &["cancelled"]).await;
    assert_eq!(row["cleanupPending"], true);
    assert!(!server.committed.is_cancelled());
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    server.release.cancel();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        server.committed.cancelled(),
    )
    .await
    .unwrap();
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/cleanup"),
        json!({}),
        Some(token),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            let (_, row) = app
                .send(
                    "GET",
                    &format!("/api/v1/uploads/{id}"),
                    Value::Null,
                    Some(token),
                )
                .await;
            if row["data"]["cleanupPending"] == false {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("late object cleanup did not reconcile");
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}
#[tokio::test]
async fn revoking_write_permission_during_remote_io_prevents_publication() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let server = mock_s3(false, false).await;
    default_s3(&app, owner, &server.endpoint).await;
    let role = role(&app, owner, "pending-writer", "self", vec![]).await;
    let (_, writer) = user(&app, owner, "pending-writer", &role).await;
    let id = upload(&app, &writer, "revoked.bin", b"revoked", None).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(&writer),
    )
    .await;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        server.received.cancelled(),
    )
    .await
    .unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{role}"),
            json!({"version":1,"permissions":["files.read"]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    server.release.cancel();
    let row = wait(&app, &writer, &id, &["failed"]).await;
    assert_eq!(row["lastError"], "PERMISSION_REVOKED");
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn an_expired_lease_is_fenced_and_a_late_worker_cannot_remove_the_published_object() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let server = mock_s3(false, false).await;
    default_s3(&app, token, &server.endpoint).await;
    let id = upload(&app, token, "lease.bin", b"lease contents", None).await;
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        server.received.cancelled(),
    )
    .await
    .unwrap();
    app.database.collection::<Document>("file_attempts").update_one(doc!{"uploadId":ObjectId::parse_str(&id).unwrap(),"kind":"publish"},doc!{"$set":{"leaseUntil":mongodb::bson::DateTime::from_millis(0),"nextRunAt":mongodb::bson::DateTime::from_millis(0)}}).await.unwrap();
    app.send(
        "POST",
        &format!("/api/v1/uploads/{id}/complete"),
        json!({}),
        Some(token),
    )
    .await;
    let done = wait(&app, token, &id, &["completed"]).await;
    server.release.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let file = done["fileId"].as_str().unwrap();
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/files/{file}/content"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        b"lease contents".as_slice()
    );
    assert_eq!(server.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"files.publish"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn file_workflow_schema_upgrade_preserves_intake_and_unknown_cleanup_kinds_stay_pending() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let id = upload(&app, token, "upgrade.bin", b"data", None).await;
    let oid = ObjectId::parse_str(&id).unwrap();
    app.database
        .collection::<Document>("file_uploads")
        .update_one(
            doc! {"_id":oid},
            doc! {"$unset":{"publishAttemptId":"","fileId":"","lastError":"","completedAt":""}},
        )
        .await
        .unwrap();
    app.database
        .collection::<Document>("file_chunks")
        .update_one(doc! {"uploadId":oid}, doc! {"$unset":{"state":""}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"files"}, doc! {"$set":{"version":1}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let (_, status) = app
        .send(
            "GET",
            &format!("/api/v1/uploads/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status["data"]["receivedIndices"], json!([0]));
    let profile =
        ObjectId::parse_str(status["data"]["storageProfileId"].as_str().unwrap()).unwrap();
    app.database.collection::<Document>("file_attempts").insert_one(doc!{"uploadId":oid,"storageProfileId":profile,"blobId":ObjectId::new(),"kind":"unknown-kind","state":"orphan","createdAt":mongodb::bson::DateTime::now()}).await.unwrap();
    app.database
        .collection::<Document>("file_uploads")
        .update_one(doc! {"_id":oid}, doc! {"$inc":{"attemptCount":1i64}})
        .await
        .unwrap();
    let cancelled = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cancel"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(cancelled.0, StatusCode::OK);
    assert_eq!(cancelled.1["data"]["cleanupPending"], true);
    let cleanup = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cleanup"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(cleanup.1["data"]["complete"], false);
    assert_eq!(
        app.database
            .collection::<Document>("file_attempts")
            .count_documents(doc! {"kind":"unknown-kind","state":"orphan"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn file_lists_use_bounded_indexes_for_all_self_category_and_name_queries() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let owner = ObjectId::parse_str(auth["user"]["id"].as_str().unwrap()).unwrap();
    let category = ObjectId::new();
    let now = mongodb::bson::DateTime::now();
    let rows=(0..2000).map(|index|doc!{"_id":ObjectId::new(),"uploadId":ObjectId::new(),"ownerId":owner,"name":format!("p{index:04}.bin"),"mediaType":"application/octet-stream","size":1i64,"sha256":"0".repeat(64),"categoryId":category,"storageProfileId":ObjectId::new(),"blobId":ObjectId::new(),"publishAttemptId":ObjectId::new(),"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("files")
        .insert_many(rows)
        .await
        .unwrap();
    for (filter, sort) in [
        (doc! {"deletedAt":null}, doc! {"createdAt":-1,"_id":-1}),
        (
            doc! {"deletedAt":null,"ownerId":owner},
            doc! {"createdAt":-1,"_id":-1},
        ),
        (
            doc! {"deletedAt":null,"name":{"$regex":"^p0"}},
            doc! {"name":1,"_id":1},
        ),
        (
            doc! {"deletedAt":null,"ownerId":owner,"categoryId":category},
            doc! {"name":1,"_id":1},
        ),
    ] {
        let result=app.database.run_command(doc!{"explain":{"find":"files","filter":filter,"sort":sort,"limit":20},"verbosity":"executionStats"}).await.unwrap();
        let stats = result.get_document("executionStats").unwrap();
        let number = |key| {
            stats
                .get_i64(key)
                .or_else(|_| stats.get_i32(key).map(i64::from))
                .unwrap()
        };
        assert_eq!(number("nReturned"), 20);
        assert!(number("totalDocsExamined") <= 20, "{stats:?}");
        assert!(number("totalKeysExamined") <= 21, "{stats:?}");
    }
    let (status, rows) = app
        .send(
            "GET",
            &format!("/api/v1/files?categoryId={category}&sort=name&pageSize=20"),
            Value::Null,
            Some(auth["accessToken"].as_str().unwrap()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows["meta"]["total"], 2000);
    assert_eq!(rows["data"].as_array().unwrap().len(), 20);
    app.cleanup().await;
}

struct ApiProcess(std::process::Child);
impl Drop for ApiProcess {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn server_process(database: &mongodb::Database, root: &std::path::Path) -> (ApiProcess, String) {
    assert!(database.name().starts_with("tri_admin_test_"));
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let values = dotenvy::from_path_iter(project.join(".env"))
        .unwrap()
        .map(Result::unwrap)
        .collect::<std::collections::HashMap<_, _>>();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let log =
        std::fs::File::create(root.join(format!("server-{}.log", uuid::Uuid::new_v4()))).unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_admin-api"));
    command
        .arg("serve")
        .current_dir(project)
        .env("TRI_MONGODB_URI", values.get("TRI_MONGODB_URI").unwrap())
        .env("TRI_MONGODB_DATABASE", database.name())
        .env("TRI_ADMIN_BIND", address.to_string())
        .env("TRI_JWT_SIGNING_KEY", support::KEY)
        .env("TRI_SETTINGS_KEYS", format!("fixture:{}", "11".repeat(32)))
        .env("TRI_SETTINGS_ACTIVE_KEY", "fixture")
        .env("TRI_STORAGE_ROOT", root)
        .stdout(std::process::Stdio::from(log.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    (
        ApiProcess(command.spawn().unwrap()),
        format!("http://{address}/api/v1"),
    )
}
async fn http(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = client
        .request(method.parse::<reqwest::Method>().unwrap(), url)
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(5));
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).unwrap());
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap(),
    )
}
async fn ready_child(
    child: &mut ApiProcess,
    client: &reqwest::Client,
    api: &str,
    token: &str,
    owner: &str,
) {
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            assert!(
                matches!(child.0.try_wait(), Ok(None)),
                "owned API process exited before readiness"
            );
            if let Ok(response) = client
                .get(format!("{api}/auth/me"))
                .bearer_auth(token)
                .timeout(std::time::Duration::from_millis(250))
                .send()
                .await
                && response.status() == StatusCode::OK
                && let Ok(bytes) = response.bytes().await
                && let Ok(row) = serde_json::from_slice::<Value>(&bytes)
                && row["data"]["id"] == owner
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("isolated child API did not authenticate the expected fixture owner");
}
#[tokio::test]
async fn a_new_server_process_recovers_a_committed_write_after_the_old_process_is_killed() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let owner = auth["user"]["id"].as_str().unwrap();
    let upstream = mock_s3(false, false).await;
    default_s3(&app, token, &upstream.endpoint).await;
    let id = upload(
        &app,
        token,
        "process-recovery.bin",
        b"process recovery contents",
        None,
    )
    .await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let (mut first, api) = server_process(&app.database, root.path());
    ready_child(&mut first, &client, &api, token, owner).await;
    assert_eq!(
        http(
            &client,
            "POST",
            &format!("{api}/uploads/{id}/complete"),
            token,
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        upstream.received.cancelled(),
    )
    .await
    .unwrap();
    first.0.kill().unwrap();
    first.0.wait().unwrap();
    app.database.collection::<Document>("file_attempts").update_one(doc!{"uploadId":ObjectId::parse_str(&id).unwrap(),"kind":"publish"},doc!{"$set":{"leaseUntil":mongodb::bson::DateTime::from_millis(0),"nextRunAt":mongodb::bson::DateTime::from_millis(0)}}).await.unwrap();
    let (mut second, api) = server_process(&app.database, root.path());
    ready_child(&mut second, &client, &api, token, owner).await;
    let file = tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            let (status, row) =
                http(&client, "GET", &format!("{api}/uploads/{id}"), token, None).await;
            assert_eq!(status, StatusCode::OK);
            if row["data"]["state"] == "completed" {
                break row["data"]["fileId"].as_str().unwrap().to_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("new main server worker did not reconcile the durable expired lease");
    let response = client
        .get(format!("{api}/files/{file}/content"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap(),
        b"process recovery contents".as_slice()
    );
    assert_eq!(upstream.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"files.publish"})
            .await
            .unwrap(),
        1
    );
    let upload = app
        .database
        .collection::<Document>("file_uploads")
        .find_one(doc! {"_id":ObjectId::parse_str(&id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let profile = upload.get_object_id("storageProfileId").unwrap();
    let orphan = ObjectId::new();
    let directory = root.path().join(".tri-upload-chunks/objects");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join(orphan.to_hex()), b"orphan").unwrap();
    let now = mongodb::bson::DateTime::now();
    app.database.collection::<Document>("file_attempts").insert_one(doc!{"uploadId":ObjectId::parse_str(&id).unwrap(),"kind":"chunk","blobId":orphan,"storageProfileId":profile,"state":"orphan","createdAt":now}).await.unwrap();
    app.database
        .collection::<Document>("file_uploads")
        .update_one(
            doc! {"_id":ObjectId::parse_str(&id).unwrap()},
            doc! {"$set":{"cleanupPending":true}},
        )
        .await
        .unwrap();
    app.database.collection::<Document>("file_cleanup_tasks").update_one(doc!{"_id":ObjectId::parse_str(&id).unwrap()},doc!{"$set":{"uploadId":ObjectId::parse_str(&id).unwrap(),"storageProfileId":profile,"state":"pending","nextRunAt":now,"updatedAt":now}}).upsert(true).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            let (_, row) = http(&client, "GET", &format!("{api}/uploads/{id}"), token, None).await;
            if !directory.join(orphan.to_hex()).exists() && row["data"]["cleanupPending"] == false {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("main server maintenance did not process the durable cleanup task");
    drop(second);
    drop(first);
    upstream.release.cancel();
    app.cleanup().await;
}

#[tokio::test]
async fn queued_publication_can_be_cancelled_without_waiting_for_busy_workers() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let upstream = mock_s3(false, false).await;
    default_s3(&app, token, &upstream.endpoint).await;
    let first = upload(&app, token, "busy-1.bin", b"first", None).await;
    let second = upload(&app, token, "busy-2.bin", b"second", None).await;
    for id in [&first, &second] {
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token),
        )
        .await;
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while upstream.writes.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let third = upload(&app, token, "queued.bin", b"third", None).await;
    let queued = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{third}/complete"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(queued.1["data"]["state"], "queued");
    let cancelled = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{third}/cancel"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(cancelled.0, StatusCode::OK);
    assert_eq!(cancelled.1["data"]["state"], "cancelled");
    assert_eq!(cancelled.1["data"]["cleanupPending"], false);
    upstream.release.cancel();
    wait(&app, token, &first, &["completed"]).await;
    wait(&app, token, &second, &["completed"]).await;
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    app.cleanup().await;
}

#[tokio::test]
#[ignore = "requires the isolated SeaweedFS fixture; run npm run test:s3"]
async fn real_s3_published_files_download_and_cleanup_through_the_application_api() {
    use admin_api::modules::files::storage::{BlobId, BlobSpec, BlobStore, StorageError};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("TRI_S3_FIXTURE_FILE").expect("run npm run test:s3")).unwrap(),
    )
    .unwrap();
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let(status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"real-file-s3","code":"real-file-s3","enabled":true,"configuration":{"kind":"s3","endpoint":fixture["endpoint"],"bucket":fixture["bucket"],"region":fixture["region"],"prefix":"api-publication","pathStyle":true},"secret":{"kind":"s3","accessKeyId":fixture["accessKeyId"],"secretAccessKey":fixture["secretAccessKey"]}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let profile = profile["data"]["id"].as_str().unwrap();
    let (_, settings) = app
        .send("GET", "/api/v1/settings/service", Value::Null, Some(token))
        .await;
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":settings["data"]["version"],"storageProfileId":profile}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let data = vec![b's'; 5 * 1024 * 1024 + 3];
    let hash = format!("{:x}", Sha256::digest(&data));
    let id = upload(&app, token, "s3-api.bin", &data, None).await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{id}/complete"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let done = wait(&app, token, &id, &["completed"]).await;
    let file = done["fileId"].as_str().unwrap();
    let stored = app
        .database
        .collection::<Document>("files")
        .find_one(doc! {"_id":ObjectId::parse_str(file).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let blob = stored.get_object_id("blobId").unwrap();
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/files/{file}/content"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), data.len() + 1)
            .await
            .unwrap(),
        data
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/files/{file}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_STORAGE_ROOT" => Some(root.path().to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    let store = BlobStore::from_profile(
        &app.database,
        Arc::new(config),
        ObjectId::parse_str(profile).unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        store
            .read(
                BlobId::parse(&blob.to_hex()).unwrap(),
                BlobSpec::new(data.len() as u64, &hash).unwrap(),
                CancellationToken::new()
            )
            .await,
        Err(StorageError::NotFound)
    ));
    app.cleanup().await;
}
