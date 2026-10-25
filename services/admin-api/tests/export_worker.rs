mod support;
use admin_api::modules::data_tools::export_jobs::maintenance;
use admin_api::{
    build_router, config::Config, modules::data_tools::export_jobs::worker, state::AppState,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{KEY, PASSWORD, TestApp};
use tower::ServiceExt;

async fn fixture() -> (TestApp, AppState, tempfile::TempDir) {
    let mut app = TestApp::new().await;
    let root = tempfile::tempdir().unwrap();
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_STORAGE_ROOT" => Some(root.path().to_str().unwrap().into()),
        _ => None,
    })
    .unwrap();
    let state = AppState::new(config, Some(app.database.clone()));
    app.router = build_router(state.clone());
    (app, state, root)
}
async fn enqueue(
    app: &TestApp,
    token: &str,
    format: &str,
    resource: &str,
    search: &str,
    max_rows: u32,
) -> Value {
    let columns = if resource == "users" {
        json!([{"field":"username","header":"账号"}])
    } else {
        json!([{"field":"key","header":"标识"},{"field":"valueType","header":"类型"},{"field":"value","header":"值"}])
    };
    let result=app.send("POST","/api/v1/data-tools/exports",json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":format,"maxRows":max_rows,"source":{"kind":"resource","resource":resource,"columns":columns,"search":search}}),Some(token)).await;
    assert_eq!(result.0, StatusCode::ACCEPTED, "{}", result.1);
    result.1["data"].clone()
}
async fn status(app: &TestApp, token: &str, id: &str) -> Value {
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
async fn content(app: &TestApp, token: &str, id: &str) -> (StatusCode, Vec<u8>) {
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
        assert!(response.headers().contains_key("etag"));
    }
    (
        code,
        to_bytes(response.into_body(), 32 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}
async fn parameters(app: &TestApp, count: usize) {
    let now = DateTime::now();
    let rows=(0..count).map(|index|doc!{"_id":ObjectId::new(),"key":format!("worker.{index:06}"),"name":"Worker fixture","group":"worker","description":"","valueType":"boolean","valueJson":if index%2==0{"true"}else{"false"},"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("parameters")
        .insert_many(rows)
        .await
        .unwrap();
}

#[tokio::test]
async fn queued_csv_and_xlsx_jobs_generate_real_large_private_files_and_authorized_downloads() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 5001).await;
    for format in ["csv", "xlsx"] {
        let job = enqueue(&app, token, format, "parameters", "worker.", 5001).await;
        let id = job["id"].as_str().unwrap();
        assert!(worker::run_once(&state).await.unwrap());
        let completed = status(&app, token, id).await;
        assert_eq!(completed["state"], "completed", "{completed}");
        assert_eq!(completed["rowCount"], 5001);
        let (code, bytes) = content(&app, token, id).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(completed["file"]["size"], bytes.len() as u64);
        assert_eq!(
            completed["file"]["sha256"],
            format!("{:x}", Sha256::digest(&bytes))
        );
        if format == "csv" {
            let rows = csv::Reader::from_reader(bytes.as_slice())
                .records()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(rows.len(), 5001);
            assert_eq!(&rows[0][0], "worker.000000");
            assert_eq!(&rows[1][2], "false");
        } else {
            use calamine::Reader;
            let mut book: calamine::Xlsx<_> =
                calamine::Xlsx::new(std::io::Cursor::new(bytes)).unwrap();
            let range = book.worksheet_range("Sheet1").unwrap();
            assert_eq!(range.height(), 5002);
            assert_eq!(range.get((2, 2)), Some(&calamine::Data::Bool(false)));
        }
        assert!(!worker::run_once(&state).await.unwrap());
    }
    assert_eq!(
        app.send("GET", "/api/v1/files", Value::Null, Some(token))
            .await
            .1["meta"]["total"],
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.complete"})
            .await
            .unwrap(),
        2
    );
    app.cleanup().await;
}

#[tokio::test]
async fn cancelled_or_over_limit_work_never_publishes_a_prefix() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 2).await;
    let cancelled = enqueue(&app, token, "csv", "parameters", "worker.", 10).await;
    let id = cancelled["id"].as_str().unwrap();
    app.send(
        "POST",
        &format!("/api/v1/data-tools/exports/{id}/cancel"),
        json!({}),
        Some(token),
    )
    .await;
    assert!(!worker::run_once(&state).await.unwrap());
    assert_eq!(content(&app, token, id).await.0, StatusCode::CONFLICT);
    let limited = enqueue(&app, token, "csv", "parameters", "worker.", 1).await;
    let id = limited["id"].as_str().unwrap();
    assert!(worker::run_once(&state).await.unwrap());
    let failed = status(&app, token, id).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["lastError"], "ROW_LIMIT");
    assert!(failed["file"].is_null());
    assert_eq!(content(&app, token, id).await.0, StatusCode::CONFLICT);
    app.cleanup().await;
}

#[tokio::test]
async fn an_unknown_database_publication_is_reconciled_before_any_regeneration() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 2).await;
    let job = enqueue(&app, token, "csv", "parameters", "worker.", 10).await;
    let id = job["id"].as_str().unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.complete"}},"validationLevel":"strict"}).await.unwrap();
    assert!(worker::run_once(&state).await.unwrap());
    assert_eq!(status(&app, token, id).await["state"], "unknown");
    let before = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    app.database
        .collection::<Document>("parameters")
        .update_many(doc! {}, doc! {"$set":{"valueJson":"false"}})
        .await
        .unwrap();
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_exports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(worker::run_once(&state).await.unwrap());
    let completed = status(&app, token, id).await;
    assert_eq!(completed["state"], "completed", "{completed}");
    assert_eq!(completed["cleanupPending"], false);
    let after = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.get_i64("generation").unwrap(),
        after.get_i64("generation").unwrap()
    );
    assert_eq!(
        before.get_object_id("candidateId").unwrap(),
        after.get_object_id("candidateId").unwrap()
    );
    let (_, bytes) = content(&app, token, id).await;
    assert!(String::from_utf8(bytes).unwrap().contains("true"));
    app.cleanup().await;
}

#[tokio::test]
async fn completed_user_exports_recheck_current_scope_before_sending_any_content() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let role=app.send("POST","/api/v1/roles",json!({"name":"Export operator","slug":"export-operator","permissions":["exports.read","exports.write","users.read"],"dataScope":"all"}),Some(owner)).await.1["data"].clone();
    let role_id = role["id"].as_str().unwrap();
    let mut subject_id = String::new();
    for name in ["download-operator", "download-subject"] {
        let created = app
            .send(
                "POST",
                "/api/v1/users",
                json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role_id]}),
                Some(owner),
            )
            .await;
        assert_eq!(created.0, StatusCode::CREATED);
        if name == "download-subject" {
            subject_id = created.1["data"]["id"].as_str().unwrap().into();
        }
    }
    let login = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"download-operator","password":PASSWORD}),
            None,
        )
        .await
        .1["data"]
        .clone();
    let token = login["accessToken"].as_str().unwrap();
    let job = enqueue(&app, token, "csv", "users", "download-", 10).await;
    let id = job["id"].as_str().unwrap();
    assert!(worker::run_once(&state).await.unwrap());
    assert_eq!(content(&app, token, id).await.0, StatusCode::OK);
    let team=app.send("POST","/api/v1/roles",json!({"name":"Download team","slug":"download-team","permissions":[],"dataScope":"self"}),Some(owner)).await.1["data"].clone();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{subject_id}"),
            json!({"version":1,"roleIds":[team["id"]]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{role_id}"),
            json!({"version":1,"dataScope":"roles","dataScopeRoleIds":[team["id"]]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(content(&app, token, id).await.0, StatusCode::OK);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{subject_id}"),
            json!({"version":2,"roleIds":[role_id]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(content(&app, token, id).await.0, StatusCode::CONFLICT);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{subject_id}"),
            json!({"version":3,"roleIds":[team["id"]]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(content(&app, token, id).await.0, StatusCode::OK);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{role_id}"),
            json!({"version":2,"dataScope":"self","dataScopeRoleIds":[]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (code, bytes) = content(&app, token, id).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains("download-subject")
    );
    assert_eq!(content(&app, owner, id).await.0, StatusCode::NOT_FOUND);
    app.cleanup().await;
}

#[tokio::test]
async fn service_lifecycle_processes_queue_and_a_fresh_service_resumes_work_after_stop() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 2).await;
    worker::stop_service(&state);
    let job = enqueue(&app, token, "csv", "parameters", "worker.", 10).await;
    let id = job["id"].as_str().unwrap();
    assert!(!worker::run_once(&state).await.unwrap());
    assert_eq!(status(&app, token, id).await["state"], "queued");
    let restarted = AppState::new((*state.config).clone(), Some(app.database.clone()));
    let service = worker::start_service(restarted.clone());
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if status(&app, token, id).await["state"] == "completed" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    worker::stop_service(&restarted);
    tokio::time::timeout(std::time::Duration::from_secs(5), service)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(content(&app, token, id).await.0, StatusCode::OK);
    app.cleanup().await;
}

#[tokio::test]
async fn expired_artifacts_are_removed_once_while_lightweight_receipts_remain() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 2).await;
    let job = enqueue(&app, token, "csv", "parameters", "worker.", 10).await;
    let id = job["id"].as_str().unwrap();
    worker::run_once(&state).await.unwrap();
    app.database
        .collection::<Document>("data_exports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_export_attempts")
        .update_many(
            doc! {"jobId":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.cleanup"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(maintenance::run_once(&state).await.unwrap().retired, 0);
    assert_eq!(status(&app, token, id).await["cleanupPending"], true);
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_export_attempts")
        .update_many(
            doc! {"jobId":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let report = maintenance::run_once(&state).await.unwrap();
    assert_eq!(report.retired, 1);
    let expired = status(&app, token, id).await;
    assert_eq!(expired["state"], "expired");
    assert_eq!(expired["cleanupPending"], false);
    assert_eq!(expired["rowCount"], 2);
    assert!(expired["file"].is_null());
    let raw = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert!(!raw.get_bool("storageRetained").unwrap());
    assert_eq!(maintenance::run_once(&state).await.unwrap().retired, 0);
    assert_eq!(content(&app, token, id).await.0, StatusCode::CONFLICT);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.cleanup"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn an_unobserved_cancelled_write_keeps_references_until_late_content_is_confirmed_and_cleaned()
 {
    use admin_api::modules::files::storage::{BlobId, BlobSpec, BlobStore, VerifiedSource};
    use futures_util::TryStreamExt;
    use tokio_util::sync::CancellationToken;
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 2).await;
    let job = enqueue(&app, token, "csv", "parameters", "worker.", 10).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.complete"}},"validationLevel":"strict"}).await.unwrap();
    worker::run_once(&state).await.unwrap();
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    let candidate = app
        .database
        .collection::<Document>("data_export_attempts")
        .find_one(doc! {"jobId":id})
        .await
        .unwrap()
        .unwrap();
    let attempt_id = candidate.get_object_id("_id").unwrap();
    let profile = candidate.get_object_id("storageProfileId").unwrap();
    let blob = BlobId::parse(&candidate.get_object_id("blobId").unwrap().to_hex()).unwrap();
    let spec = BlobSpec::new(
        candidate.get_i64("size").unwrap() as u64,
        candidate.get_str("sha256").unwrap(),
    )
    .unwrap();
    let store = BlobStore::from_profile(&app.database, state.config.clone(), profile)
        .await
        .unwrap();
    let chunks = store
        .read(blob, spec.clone(), CancellationToken::new())
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let bytes = chunks.into_iter().flatten().collect::<Vec<_>>();
    store.delete(blob, CancellationToken::new()).await.unwrap();
    app.database.collection::<Document>("data_export_attempts").update_one(doc!{"_id":attempt_id},doc!{"$set":{"state":"putting","blobKnown":false,"nextRunAt":DateTime::from_millis(0)}}).await.unwrap();
    app.send(
        "POST",
        &format!("/api/v1/data-tools/exports/{id}/cancel"),
        json!({}),
        Some(token),
    )
    .await;
    assert_eq!(maintenance::run_once(&state).await.unwrap().retired, 0);
    assert_eq!(
        status(&app, token, &id.to_hex()).await["cleanupPending"],
        true
    );
    let source = VerifiedSource::stage(
        &state.config,
        std::io::Cursor::new(bytes),
        spec,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    store
        .put(blob, source, CancellationToken::new())
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_export_attempts")
        .update_one(
            doc! {"_id":attempt_id},
            doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert_eq!(maintenance::run_once(&state).await.unwrap().retired, 1);
    assert_eq!(
        status(&app, token, &id.to_hex()).await["cleanupPending"],
        false
    );
    app.cleanup().await;
}

#[tokio::test]
async fn cleanup_lease_fences_recovery_until_it_is_released_or_expired() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 2).await;
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.complete"}},"validationLevel":"strict"}).await.unwrap();
    let job = enqueue(&app, token, "csv", "parameters", "worker.", 10).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    worker::run_once(&state).await.unwrap();
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_exports")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    app.database.collection::<Document>("data_export_attempts").update_one(doc!{"jobId":id},doc!{"$set":{"cleanupToken":"other-cleaner","cleanupExpiresAt":DateTime::from_millis(DateTime::now().timestamp_millis()+60000)}}).await.unwrap();
    assert!(!worker::run_once(&state).await.unwrap());
    assert_eq!(status(&app, token, &id.to_hex()).await["state"], "unknown");
    app.database
        .collection::<Document>("data_export_attempts")
        .update_one(
            doc! {"jobId":id},
            doc! {"$set":{"cleanupExpiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(worker::run_once(&state).await.unwrap());
    assert_eq!(
        status(&app, token, &id.to_hex()).await["state"],
        "completed"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn cleanup_does_not_touch_the_current_generation_while_its_execution_lease_is_live() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 2).await;
    let job = enqueue(&app, token, "csv", "parameters", "worker.", 1).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    worker::run_once(&state).await.unwrap();
    app.database.collection::<Document>("data_exports").update_one(doc!{"_id":id},doc!{"$set":{"state":"running","leaseToken":"resuming-writer","leaseExpiresAt":DateTime::from_millis(DateTime::now().timestamp_millis()+60000)}}).await.unwrap();
    app.database.collection::<Document>("data_export_attempts").update_one(doc!{"jobId":id},doc!{"$set":{"state":"generating","workspaceCleaned":false,"nextRunAt":DateTime::from_millis(0)}}).await.unwrap();
    assert_eq!(maintenance::run_once(&state).await.unwrap().scanned, 0);
    assert!(
        !app.database
            .collection::<Document>("data_export_attempts")
            .find_one(doc! {"jobId":id})
            .await
            .unwrap()
            .unwrap()
            .get_bool("workspaceCleaned")
            .unwrap()
    );
    app.cleanup().await;
}

#[tokio::test]
async fn cleanup_receipts_are_not_resurrected_when_no_write_recovery_requeues_work() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameters(&app, 2).await;
    let job = enqueue(&app, token, "csv", "parameters", "worker.", 1).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    worker::run_once(&state).await.unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.cleanup"})
            .await
            .unwrap(),
        1
    );
    app.database
        .collection::<Document>("data_exports")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"state":"unknown","nextRunAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(worker::run_once(&state).await.unwrap());
    assert_eq!(status(&app, token, &id.to_hex()).await["state"], "queued");
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.cleanup"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn large_dictionary_item_exports_keep_parent_filters_and_global_typed_sorting_bounded() {
    let (app, state, _root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let parent = app
        .send(
            "POST",
            "/api/v1/dictionaries",
            json!({"name":"Async parent","key":"async.keep"}),
            Some(token),
        )
        .await
        .1["data"]
        .clone();
    let parent_id = ObjectId::parse_str(parent["id"].as_str().unwrap()).unwrap();
    let deleted = ObjectId::new();
    let now = DateTime::now();
    app.database.collection::<Document>("dictionaries").insert_one(doc!{"_id":deleted,"key":"async.keep","name":"Removed parent","description":"","enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":now}).await.unwrap();
    let mut rows=(0..2501i32).map(|index|doc!{"_id":ObjectId::new(),"dictionaryId":parent_id,"label":format!("Item{index}"),"value":format!("v{index}"),"sort":index,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    for (dictionary, value, enabled) in [
        (parent_id, "disabled", false),
        (deleted, "deleted-parent", true),
    ] {
        rows.push(doc!{"_id":ObjectId::new(),"dictionaryId":dictionary,"label":value,"value":value,"sort":100000i32,"enabled":enabled,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null});
    }
    app.database
        .collection::<Document>("dictionary_items")
        .insert_many(rows)
        .await
        .unwrap();
    let result=app.send("POST","/api/v1/data-tools/exports",json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":"csv","maxRows":3000,"source":{"kind":"resource","resource":"dictionaryItems","columns":[{"field":"dictionaryKey","header":"parent"},{"field":"value","header":"value"},{"field":"sort","header":"sort"}],"filters":[{"field":"dictionaryKey","op":"eq","value":"async.keep"},{"field":"enabled","op":"eq","value":true}],"sort":{"field":"sort","direction":"desc"}}}),Some(token)).await;
    assert_eq!(result.0, StatusCode::ACCEPTED, "{}", result.1);
    let id = result.1["data"]["id"].as_str().unwrap();
    worker::run_once(&state).await.unwrap();
    assert_eq!(status(&app, token, id).await["rowCount"], 2501);
    let (code, bytes) = content(&app, token, id).await;
    assert_eq!(code, StatusCode::OK);
    let records = csv::Reader::from_reader(bytes.as_slice())
        .records()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(records.len(), 2501);
    assert_eq!(&records[0][1], "v2500");
    assert_eq!(&records[0][2], "2500");
    assert!(!String::from_utf8(bytes).unwrap().contains("deleted-parent"));
    app.cleanup().await;
}

#[tokio::test]
async fn unavailable_workspace_defers_the_queue_without_altering_existing_files() {
    let (app, state, root) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let job = enqueue(&app, token, "csv", "parameters", "", 10).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    let blocked = root.path().join("existing-file");
    std::fs::write(&blocked, b"preserve").unwrap();
    let mut config = (*state.config).clone();
    config.storage_root = blocked.to_str().unwrap().into();
    let unavailable = AppState::new(config, Some(app.database.clone()));
    assert!(worker::run_once(&unavailable).await.is_err());
    let current = status(&app, token, &id.to_hex()).await;
    assert_eq!(current["state"], "queued");
    assert_eq!(current["lastError"], "WORKSPACE_UNAVAILABLE");
    let raw = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert!(raw.get_datetime("nextRunAt").unwrap() > &DateTime::now());
    assert_eq!(std::fs::read(blocked).unwrap(), b"preserve");
    app.cleanup().await;
}
