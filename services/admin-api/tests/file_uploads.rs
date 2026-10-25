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
fn input(name: &str, size: u64) -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":name,"mediaType":"application/octet-stream","size":size,"sha256":format!("{:x}",Sha256::digest(b"fixture"))})
}
async fn create(app: &TestApp, token: &str, body: Value) -> Value {
    let (status, row) = app.send("POST", "/api/v1/uploads", body, Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{row}");
    row["data"].clone()
}
async fn chunk(
    app: &TestApp,
    token: &str,
    id: &str,
    index: u64,
    data: &[u8],
    hash: Option<&str>,
    content_type: &str,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/uploads/{id}/chunks/{index}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", content_type);
    if let Some(hash) = hash {
        request = request.header("x-chunk-sha256", hash)
    }
    let response = app
        .router
        .clone()
        .oneshot(request.body(Body::from(data.to_vec())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
#[tokio::test]
async fn categories_are_versioned_unique_and_referenced_uploads_block_deletion() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (status, row) = app
        .send(
            "POST",
            "/api/v1/file-categories",
            json!({"name":"图片","code":"images","description":"资料图片"}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{row}");
    let id = row["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/file-categories",
            json!({"name":"重复","code":"images"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, updated) = app
        .send(
            "PATCH",
            &format!("/api/v1/file-categories/{id}"),
            json!({"version":1,"name":"图片资料"}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["data"]["version"], 2);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/file-categories/{id}"),
            json!({"version":1,"name":"旧修改"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut body = input("资料.bin", 1);
    body["categoryId"] = json!(id);
    let upload = create(&app, token, body).await;
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/file-categories/{id}?version=2"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/uploads/{}/cancel", upload["id"].as_str().unwrap()),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/file-categories/{id}?version=2"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let list = app
        .send(
            "GET",
            "/api/v1/file-categories?search=ima&sort=code",
            Value::Null,
            Some(token),
        )
        .await
        .1;
    assert_eq!(list["meta"]["total"], 0);
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/file-categories",
            json!({"name":"新图片","code":"images"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    app.cleanup().await;
}
#[tokio::test]
async fn upload_requests_replay_exactly_and_concurrent_creation_has_one_identity() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let body = input("same.bin", 5 * 1024 * 1024);
    let (a, b) = tokio::join!(
        app.send("POST", "/api/v1/uploads", body.clone(), Some(token)),
        app.send("POST", "/api/v1/uploads", body.clone(), Some(token))
    );
    assert_eq!(a.0, StatusCode::CREATED, "{}", a.1);
    assert_eq!(b.0, StatusCode::CREATED, "{}", b.1);
    assert_eq!(a.1["data"]["id"], b.1["data"]["id"]);
    assert_eq!(a.1["data"]["chunkSize"], 4 * 1024 * 1024);
    assert_eq!(a.1["data"]["totalChunks"], 2);
    let mut changed = body.clone();
    changed["name"] = json!("different.bin");
    assert_eq!(
        app.send("POST", "/api/v1/uploads", changed, Some(token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.database
            .collection::<Document>("file_uploads")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"uploads.create"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn upload_inputs_reject_unsafe_names_sizes_hashes_and_missing_references() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    for name in [
        "../a",
        "C:\\secret",
        "CON.txt",
        "trailing.",
        "line\nname",
        " ",
    ] {
        assert_eq!(
            app.send("POST", "/api/v1/uploads", input(name, 1), Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name}"
        )
    }
    for (key, value) in [
        ("size", json!(5u64 * 1024 * 1024 * 1024 + 1)),
        ("sha256", json!("bad")),
        ("mediaType", json!("not a type")),
        ("requestId", json!("bad")),
        ("categoryId", json!(ObjectId::new().to_hex())),
        ("storageProfileId", json!(ObjectId::new().to_hex())),
    ] {
        let mut body = input("valid.bin", 1);
        body[key] = value;
        assert_eq!(
            app.send("POST", "/api/v1/uploads", body, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{key}"
        )
    }
    let empty = create(&app, token, input("empty.bin", 0)).await;
    assert_eq!(empty["totalChunks"], 0);
    app.cleanup().await;
}
#[tokio::test]
async fn active_uploads_pin_storage_until_cancellation_and_cleanup_finish() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let(_,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"上传存储","code":"uploads-storage","enabled":true,"configuration":{"kind":"localStorage","subdirectory":"uploads-target"}}),Some(token)).await;
    let id = profile["data"]["id"].as_str().unwrap();
    let mut body = input("file.bin", 1);
    body["storageProfileId"] = json!(id);
    let upload = create(&app, token, body).await;
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{id}"),
            json!({"version":1,"enabled":false}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/settings/profiles/{id}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let path = format!("/api/v1/uploads/{}/cancel", upload["id"].as_str().unwrap());
    let first = app.send("POST", &path, json!({}), Some(token)).await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(first.1["data"]["state"], "cancelled");
    assert_eq!(
        app.send("POST", &path, json!({}), Some(token)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/settings/profiles/{id}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    app.cleanup().await;
}
#[tokio::test]
async fn chunk_receipts_are_persistent_idempotent_and_cancelled_content_is_cleaned() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let data = b"chunk data";
    let hash = format!("{:x}", Sha256::digest(data));
    let upload = create(&app, token, input("chunk.bin", data.len() as u64)).await;
    let id = upload["id"].as_str().unwrap();
    let first = chunk(
        &app,
        token,
        id,
        0,
        data,
        Some(&hash),
        "application/octet-stream",
    )
    .await;
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            data,
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
    );
    let status = app
        .send(
            "GET",
            &format!("/api/v1/uploads/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status.1["data"]["receivedIndices"], json!([0]));
    assert_eq!(status.1["data"]["receivedBytes"], data.len());
    assert_eq!(
        app.database
            .collection::<Document>("file_chunks")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let cancelled = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cancel"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(cancelled.0, StatusCode::OK);
    assert_eq!(
        cancelled.1["data"]["cleanupPending"], false,
        "{}",
        cancelled.1
    );
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            data,
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        std::fs::read_dir(root.path().join(".tri-upload-chunks/objects"))
            .unwrap()
            .count(),
        0
    );
    app.cleanup().await;
}

async fn other(app: &TestApp, owner: &str) -> String {
    let(_,role)=app.send("POST","/api/v1/roles",json!({"name":"file-user","slug":"file-user","permissions":["files.read","files.write"],"dataScope":"all"}),Some(owner)).await;
    let(status,_)=app.send("POST","/api/v1/users",json!({"username":"file-user","displayName":"Files","password":support::PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED);
    app.send(
        "POST",
        "/api/v1/auth/login",
        json!({"username":"file-user","password":support::PASSWORD}),
        None,
    )
    .await
    .1["data"]["accessToken"]
        .as_str()
        .unwrap()
        .into()
}

#[tokio::test]
async fn upload_sessions_are_owner_bound_even_for_other_all_scope_users() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let first = create(&app, owner, input("owner.bin", 1)).await;
    let second_token = other(&app, owner).await;
    let second = create(&app, &second_token, input("other.bin", 1)).await;
    let id = first["id"].as_str().unwrap();
    for path in [
        format!("/api/v1/uploads/{id}"),
        format!("/api/v1/uploads/{id}/cancel"),
        format!("/api/v1/uploads/{id}/cleanup"),
    ] {
        let method = if path.ends_with(id) { "GET" } else { "POST" };
        assert_eq!(
            app.send(method, &path, json!({}), Some(&second_token))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let hash = format!("{:x}", Sha256::digest(b"a"));
    assert_eq!(
        chunk(
            &app,
            &second_token,
            id,
            0,
            b"a",
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let list = app
        .send(
            "GET",
            "/api/v1/uploads?sort=name&pageSize=1",
            Value::Null,
            Some(&second_token),
        )
        .await
        .1;
    assert_eq!(list["meta"]["total"], 1);
    assert_eq!(list["data"][0]["id"], second["id"]);
    assert_eq!(
        app.send("GET", "/api/v1/uploads", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn chunks_validate_transport_digest_limits_and_have_one_concurrent_winner() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let upload = create(&app, token, input("race.bin", 4)).await;
    let id = upload["id"].as_str().unwrap();
    let first = format!("{:x}", Sha256::digest(b"aaaa"));
    let second = format!("{:x}", Sha256::digest(b"bbbb"));
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            b"aaaa",
            None,
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            b"aaaa",
            Some(&first),
            "application/json"
        )
        .await
        .0,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            1,
            b"aaaa",
            Some(&first),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            b"bbbb",
            Some(&first),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let over = vec![0; 4 * 1024 * 1024 + 1];
    let hash = format!("{:x}", Sha256::digest(&over));
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            &over,
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let (a, b) = tokio::join!(
        chunk(
            &app,
            token,
            id,
            0,
            b"aaaa",
            Some(&first),
            "application/octet-stream"
        ),
        chunk(
            &app,
            token,
            id,
            0,
            b"bbbb",
            Some(&second),
            "application/octet-stream"
        )
    );
    assert!(
        matches!(
            (a.0, b.0),
            (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
        ),
        "{} / {}",
        a.1,
        b.1
    );
    let result = app
        .send(
            "GET",
            &format!("/api/v1/uploads/{id}"),
            Value::Null,
            Some(token),
        )
        .await
        .1;
    assert_eq!(result["data"]["receivedCount"], 1);
    assert_eq!(result["data"]["receivedBytes"], 4);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"uploads.chunk.accept"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn rejected_metadata_commit_rolls_back_receipts_and_cleans_only_the_candidate() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let upload = create(&app, token, input("rollback.bin", 4)).await;
    let id = upload["id"].as_str().unwrap();
    let hash = format!("{:x}", Sha256::digest(b"data"));
    app.database.run_command(doc!{"collMod":"file_chunks","validator":{"forceRejected":{"$exists":true}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            b"data",
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("file_chunks")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"uploads.chunk.accept"})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        std::fs::read_dir(root.path().join(".tri-upload-chunks/objects"))
            .unwrap()
            .count(),
        0
    );
    app.database
        .run_command(doc! {"collMod":"file_chunks","validator":{}})
        .await
        .unwrap();
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            b"data",
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn cleanup_failure_retains_durable_pending_state_and_can_be_retried() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let upload = create(&app, token, input("cleanup.bin", 4)).await;
    let id = upload["id"].as_str().unwrap();
    let hash = format!("{:x}", Sha256::digest(b"data"));
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            b"data",
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
    );
    app.database
        .run_command(doc! {"collMod":"file_attempts","validator":{"state":{"$ne":"cleaned"}}})
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
    app.database
        .run_command(doc! {"collMod":"file_attempts","validator":{}})
        .await
        .unwrap();
    let retry = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cleanup"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(retry.0, StatusCode::OK);
    assert_eq!(retry.1["data"]["complete"], true);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"uploads.cleanup"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn active_upload_quota_allows_replay_and_is_released_by_cancel() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let body = input("quota.bin", 1);
    let first = create(&app, token, body.clone()).await;
    for _ in 1..32 {
        create(&app, token, input("quota.bin", 1)).await;
    }
    assert_eq!(
        app.send("POST", "/api/v1/uploads", body, Some(token))
            .await
            .0,
        StatusCode::CREATED
    );
    let next = input("next.bin", 1);
    assert_eq!(
        app.send("POST", "/api/v1/uploads", next.clone(), Some(token))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    app.send(
        "POST",
        &format!("/api/v1/uploads/{}/cancel", first["id"].as_str().unwrap()),
        json!({}),
        Some(token),
    )
    .await;
    assert_eq!(
        app.send("POST", "/api/v1/uploads", next, Some(token))
            .await
            .0,
        StatusCode::CREATED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn wildcard_media_types_are_rejected_as_file_metadata() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut body = input("mime.bin", 1);
    body["mediaType"] = json!("text/*");
    assert_eq!(
        app.send("POST", "/api/v1/uploads", body, Some(token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.cleanup().await;
}

#[tokio::test]
async fn receiving_status_and_chunk_identity_survive_a_new_service_state() {
    let (mut app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap().to_owned();
    let row = create(&app, &token, input("restart.bin", 4)).await;
    let id = row["id"].as_str().unwrap();
    let hash = format!("{:x}", Sha256::digest(b"data"));
    assert_eq!(
        chunk(
            &app,
            &token,
            id,
            0,
            b"data",
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
    );
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_STORAGE_ROOT" => Some(root.path().to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    app.router = build_router(AppState::new(config, Some(app.database.clone())));
    let status = app
        .send(
            "GET",
            &format!("/api/v1/uploads/{id}"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(status.0, StatusCode::OK);
    assert_eq!(status.1["data"]["receivedIndices"], json!([0]));
    assert_eq!(
        chunk(
            &app,
            &token,
            id,
            0,
            b"data",
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
    );
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
async fn cancellation_racing_with_chunk_io_never_leaves_an_accepted_orphan() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = create(&app, token, input("cancel-race.bin", 1024 * 1024)).await;
    let id = row["id"].as_str().unwrap();
    let data = vec![b'x'; 1024 * 1024];
    let hash = format!("{:x}", Sha256::digest(&data));
    let cancel_path = format!("/api/v1/uploads/{id}/cancel");
    let (write, cancel) = tokio::join!(
        chunk(
            &app,
            token,
            id,
            0,
            &data,
            Some(&hash),
            "application/octet-stream"
        ),
        app.send("POST", &cancel_path, json!({}), Some(token))
    );
    assert!(matches!(write.0, StatusCode::OK | StatusCode::CONFLICT));
    assert_eq!(cancel.0, StatusCode::OK);
    let cleanup = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cleanup"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(cleanup.0, StatusCode::OK);
    assert_eq!(cleanup.1["data"]["complete"], true, "{}", cleanup.1);
    if root.path().join(".tri-upload-chunks/objects").exists() {
        assert_eq!(
            std::fs::read_dir(root.path().join(".tri-upload-chunks/objects"))
                .unwrap()
                .count(),
            0
        )
    }
    app.cleanup().await;
}

#[tokio::test]
async fn cancelled_writers_remain_pending_instead_of_being_declared_cleaned() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let upload = create(&app, token, input("pending.bin", 4)).await;
    let id = ObjectId::parse_str(upload["id"].as_str().unwrap()).unwrap();
    let profile = ObjectId::parse_str(upload["storageProfileId"].as_str().unwrap()).unwrap();
    app.database.collection::<Document>("file_attempts").insert_one(doc!{"uploadId":id,"blobId":ObjectId::new(),"storageProfileId":profile,"kind":"chunk","state":"writing","leaseUntil":mongodb::bson::DateTime::from_millis(mongodb::bson::DateTime::now().timestamp_millis()+300000),"createdAt":mongodb::bson::DateTime::now()}).await.unwrap();
    app.database
        .collection::<Document>("file_uploads")
        .update_one(doc! {"_id":id}, doc! {"$set":{"attemptCount":1i64}})
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
    let result = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cleanup"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(result.1["data"]["pending"], 1);
    assert_eq!(result.1["data"]["complete"], false);
    app.cleanup().await;
}

#[tokio::test]
async fn replay_retains_the_original_default_storage_after_defaults_change() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let body = input("default.bin", 1);
    let original = create(&app, token, body.clone()).await;
    let(_,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"新默认存储","code":"new-default","enabled":true,"configuration":{"kind":"localStorage","subdirectory":"new-default"}}),Some(token)).await;
    let (_, service) = app
        .send("GET", "/api/v1/settings/service", Value::Null, Some(token))
        .await;
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":service["data"]["version"],"storageProfileId":profile["data"]["id"]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let replay = create(&app, token, body).await;
    assert_eq!(replay["id"], original["id"]);
    assert_eq!(replay["storageProfileId"], original["storageProfileId"]);
    let new = create(&app, token, input("new.bin", 1)).await;
    assert_eq!(new["storageProfileId"], profile["data"]["id"]);
    app.cleanup().await;
}

#[tokio::test]
async fn reference_creation_and_category_or_profile_deletion_cannot_both_succeed() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (_, category) = app
        .send(
            "POST",
            "/api/v1/file-categories",
            json!({"name":"并发分类","code":"race"}),
            Some(token),
        )
        .await;
    let id = category["data"]["id"].as_str().unwrap();
    let mut body = input("category.bin", 1);
    body["categoryId"] = json!(id);
    let delete_path = format!("/api/v1/file-categories/{id}?version=1");
    let (a, b) = tokio::join!(
        app.send("POST", "/api/v1/uploads", body, Some(token)),
        app.send("DELETE", &delete_path, Value::Null, Some(token))
    );
    assert!(
        matches!(
            (a.0, b.0),
            (StatusCode::CREATED, StatusCode::CONFLICT)
                | (StatusCode::UNPROCESSABLE_ENTITY, StatusCode::NO_CONTENT)
        ),
        "{} / {}",
        a.1,
        b.1
    );
    let(_,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"并发存储","code":"race-storage","enabled":true,"configuration":{"kind":"localStorage","subdirectory":"race-storage"}}),Some(token)).await;
    let id = profile["data"]["id"].as_str().unwrap();
    let mut body = input("profile.bin", 1);
    body["storageProfileId"] = json!(id);
    let delete_path = format!("/api/v1/settings/profiles/{id}?version=1");
    let (a, b) = tokio::join!(
        app.send("POST", "/api/v1/uploads", body, Some(token)),
        app.send("DELETE", &delete_path, Value::Null, Some(token))
    );
    assert!(
        matches!(
            (a.0, b.0),
            (StatusCode::CREATED, StatusCode::CONFLICT)
                | (StatusCode::UNPROCESSABLE_ENTITY, StatusCode::NO_CONTENT)
        ),
        "{} / {}",
        a.1,
        b.1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn upload_schema_is_explicit_and_unknown_versions_are_rejected() {
    let (app, _root) = app().await;
    app.database
        .collection::<Document>("_schema_migrations")
        .delete_one(doc! {"_id":"files"})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"files"}, doc! {"$set":{"version":999}})
        .await
        .unwrap();
    assert!(matches!(
        admin_api::storage::schema::migrate_database(&app.database).await,
        Err(admin_api::common::error::AppError::SchemaMismatch)
    ));
    app.cleanup().await;
}

#[tokio::test]
async fn chunk_body_concurrency_is_bounded_and_aborted_reads_release_capacity() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let upload = create(&app, token, input("slots.bin", 4)).await;
    let id = upload["id"].as_str().unwrap();
    let hash = format!("{:x}", Sha256::digest(b"data"));
    let entered = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let entered = entered.clone();
        let body = Body::from_stream(futures_util::stream::once(async move {
            entered.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
        }));
        let request = Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/uploads/{id}/chunks/0"))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/octet-stream")
            .header("x-chunk-sha256", &hash)
            .body(body)
            .unwrap();
        let router = app.router.clone();
        workers.push(tokio::spawn(async move {
            router.oneshot(request).await.unwrap()
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while entered.load(Ordering::SeqCst) < 8 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await
        }
    })
    .await
    .unwrap();
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            b"data",
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    for worker in workers {
        worker.abort();
        let _ = worker.await;
    }
    assert_eq!(
        app.database
            .collection::<Document>("file_attempts")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            b"data",
            Some(&hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn category_and_upload_pages_use_the_deployed_bounded_indexes() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let owner = ObjectId::parse_str(auth["user"]["id"].as_str().unwrap()).unwrap();
    let other = ObjectId::new();
    let profile = ObjectId::new();
    let now = mongodb::bson::DateTime::now();
    let categories=(0..2000).map(|index|doc!{"name":format!("p{index:04}"),"code":format!("p{index:04}"),"description":"","version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("file_categories")
        .insert_many(categories)
        .await
        .unwrap();
    let uploads=(0..2000).map(|index|doc!{"ownerId":if index%2==0{owner}else{other},"requestId":uuid::Uuid::new_v4().to_string(),"requestHash":"fixture","name":format!("p{index:04}.bin"),"mediaType":"application/octet-stream","size":1i64,"sha256":"0".repeat(64),"categoryId":null,"storageProfileId":profile,"state":"cancelled","chunkSize":4194304i64,"totalChunks":1i64,"receivedCount":0i64,"receivedBytes":0i64,"attemptCount":0i64,"cleanupPending":false,"version":1i64,"createdAt":now,"updatedAt":now}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("file_uploads")
        .insert_many(uploads)
        .await
        .unwrap();
    for (collection, filter, sort) in [
        (
            "file_categories",
            doc! {"deletedAt":null},
            doc! {"createdAt":-1,"_id":-1},
        ),
        (
            "file_categories",
            doc! {"deletedAt":null,"code":{"$regex":"^p0"}},
            doc! {"code":1,"_id":1},
        ),
        (
            "file_categories",
            doc! {"deletedAt":null},
            doc! {"name":1,"_id":1},
        ),
        (
            "file_uploads",
            doc! {"ownerId":owner},
            doc! {"createdAt":-1,"_id":-1},
        ),
        (
            "file_uploads",
            doc! {"ownerId":owner,"name":{"$regex":"^p0"}},
            doc! {"name":1,"_id":1},
        ),
    ] {
        let result=app.database.run_command(doc!{"explain":{"find":collection,"filter":filter,"sort":sort,"limit":20},"verbosity":"executionStats"}).await.unwrap();
        let stats = result.get_document("executionStats").unwrap();
        let number = |key| {
            stats
                .get_i64(key)
                .or_else(|_| stats.get_i32(key).map(i64::from))
                .unwrap()
        };
        assert_eq!(number("nReturned"), 20);
        assert!(number("totalDocsExamined") <= 20, "{collection}: {stats:?}");
        assert!(number("totalKeysExamined") <= 21, "{collection}: {stats:?}");
    }
    let token = auth["accessToken"].as_str().unwrap();
    let listed = app
        .send(
            "GET",
            "/api/v1/uploads?pageSize=20&sort=name",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(listed.1["meta"]["total"], 1000);
    assert_eq!(listed.1["data"].as_array().unwrap().len(), 20);
    app.cleanup().await;
}

#[tokio::test]
async fn out_of_order_full_size_and_tail_chunks_have_one_persistent_receipt_each() {
    let (app, _root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let full = vec![b'a'; 4 * 1024 * 1024];
    let tail = b"end";
    let full_hash = format!("{:x}", Sha256::digest(&full));
    let tail_hash = format!("{:x}", Sha256::digest(tail));
    let upload = create(&app, token, input("ordered.bin", full.len() as u64 + 3)).await;
    let id = upload["id"].as_str().unwrap();
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            1,
            tail,
            Some(&tail_hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            0,
            &full,
            Some(&full_hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        chunk(
            &app,
            token,
            id,
            1,
            tail,
            Some(&tail_hash),
            "application/octet-stream"
        )
        .await
        .0,
        StatusCode::OK
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
    assert_eq!(status["data"]["receivedIndices"], json!([0, 1]));
    assert_eq!(status["data"]["receivedBytes"], full.len() + 3);
    assert_eq!(status["data"]["receivedCount"], 2);
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
async fn cleanup_is_bounded_to_one_hundred_owned_candidates_and_replays_converge() {
    let (app, root) = app().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let upload = create(&app, token, input("batch.bin", 1)).await;
    let id = ObjectId::parse_str(upload["id"].as_str().unwrap()).unwrap();
    let profile = ObjectId::parse_str(upload["storageProfileId"].as_str().unwrap()).unwrap();
    let now = mongodb::bson::DateTime::now();
    app.database.collection::<Document>("file_attempts").insert_many((0..101).map(|_|doc!{"uploadId":id,"storageProfileId":profile,"blobId":ObjectId::new(),"kind":"chunk","state":"orphan","createdAt":now}).collect::<Vec<_>>()).await.unwrap();
    app.database
        .collection::<Document>("file_uploads")
        .update_one(doc! {"_id":id}, doc! {"$set":{"attemptCount":101i64}})
        .await
        .unwrap();
    let directory = root.path().join(".tri-upload-chunks/objects");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("unrelated.txt"), b"preserve").unwrap();
    let first = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cancel"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(first.1["data"]["cleanupPending"], true);
    assert_eq!(
        app.database
            .collection::<Document>("file_attempts")
            .count_documents(doc! {"state":"cleaned"})
            .await
            .unwrap(),
        100
    );
    let second = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cleanup"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(second.1["data"]["processed"], 1);
    assert_eq!(second.1["data"]["complete"], true);
    let replay = app
        .send(
            "POST",
            &format!("/api/v1/uploads/{id}/cleanup"),
            json!({}),
            Some(token),
        )
        .await;
    assert_eq!(replay.1["data"]["processed"], 0);
    assert_eq!(
        std::fs::read(directory.join("unrelated.txt")).unwrap(),
        b"preserve"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn browser_chunk_preflights_allow_the_digest_header_only_for_configured_origins() {
    let (app, _root) = app().await;
    for (origin, allowed) in [
        ("http://localhost:1420", true),
        ("https://unconfigured.example.test", false),
    ] {
        let request = Request::builder()
            .method("OPTIONS")
            .uri("/api/v1/uploads/000000000000000000000000/chunks/0")
            .header("origin", origin)
            .header("access-control-request-method", "PUT")
            .header(
                "access-control-request-headers",
                "authorization,content-type,x-chunk-sha256",
            )
            .body(Body::empty())
            .unwrap();
        let response = app.router.clone().oneshot(request).await.unwrap();
        if allowed {
            assert_eq!(response.headers()["access-control-allow-origin"], origin);
            assert!(
                response.headers()["access-control-allow-headers"]
                    .to_str()
                    .unwrap()
                    .contains("x-chunk-sha256")
            );
        } else {
            assert!(
                !response
                    .headers()
                    .contains_key("access-control-allow-origin")
            );
        }
    }
    app.cleanup().await;
}
