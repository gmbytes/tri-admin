mod support;
use admin_api::{
    build_router,
    config::Config,
    modules::{
        data_tools::export_jobs::worker,
        files::storage::{BlobId, BlobStore},
    },
    state::AppState,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{KEY, PASSWORD, TestApp};
use tower::ServiceExt;
async fn fixture() -> (TestApp, AppState, tempfile::TempDir, Value) {
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
    let auth = app.login().await;
    (app, state, root, auth)
}
async fn parameter(app: &TestApp, token: &str, key: &str) -> Value {
    let result = app
        .send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"Retry","key":key,"valueType":"boolean","value":true}),
            Some(token),
        )
        .await;
    assert_eq!(result.0, StatusCode::CREATED);
    result.1["data"].clone()
}
async fn queue(app: &TestApp, token: &str, resource: &str, max_rows: u32) -> Value {
    let field = if resource == "users" {
        "username"
    } else {
        "key"
    };
    let result=app.send("POST","/api/v1/data-tools/exports",json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":"csv","maxRows":max_rows,"source":{"kind":"resource","resource":resource,"columns":[{"field":field,"header":field}],"search":"retry."}}),Some(token)).await;
    assert_eq!(result.0, StatusCode::ACCEPTED);
    result.1["data"].clone()
}
async fn read(app: &TestApp, token: &str, id: &str) -> Value {
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
async fn retry(app: &TestApp, token: &str, id: &str, request: &str) -> (StatusCode, Value) {
    app.send(
        "POST",
        &format!("/api/v1/data-tools/exports/{id}/retry"),
        json!({"requestId":request}),
        Some(token),
    )
    .await
}
#[tokio::test]
async fn retry_receipts_are_idempotent_even_when_the_job_fails_again_before_acknowledgement() {
    let (app, state, _root, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameter(&app, token, "retry.one").await;
    let extra = parameter(&app, token, "retry.two").await;
    let job = queue(&app, token, "parameters", 1).await;
    let id = job["id"].as_str().unwrap();
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, id).await["state"], "failed");
    let request = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        retry(&app, token, id, &request).await.0,
        StatusCode::ACCEPTED
    );
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, id).await["state"], "failed");
    let before = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap()
        .get_i64("generation")
        .unwrap();
    assert_eq!(retry(&app, token, id, &request).await.0, StatusCode::OK);
    assert_eq!(read(&app, token, id).await["state"], "failed");
    let receipt = app
        .send(
            "GET",
            &format!("/api/v1/data-tools/exports/{id}/actions/{request}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(receipt.0, StatusCode::OK);
    assert_eq!(receipt.1["data"]["kind"], "retry");
    assert_eq!(
        app.database
            .collection::<Document>("data_exports")
            .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
            .await
            .unwrap()
            .unwrap()
            .get_i64("generation")
            .unwrap(),
        before
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.retry"})
            .await
            .unwrap(),
        1
    );
    app.send(
        "DELETE",
        &format!(
            "/api/v1/parameters/{}?version=1",
            extra["id"].as_str().unwrap()
        ),
        Value::Null,
        Some(token),
    )
    .await;
    assert_eq!(
        retry(&app, token, id, &uuid::Uuid::new_v4().to_string())
            .await
            .0,
        StatusCode::ACCEPTED
    );
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, id).await["state"], "completed");
    app.cleanup().await;
}
#[tokio::test]
async fn unknown_missing_content_is_not_regenerated_until_an_explicit_retry_and_old_candidates_remain_tracked()
 {
    let (app, state, _root, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameter(&app, token, "retry.one").await;
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.complete"}},"validationLevel":"strict"}).await.unwrap();
    let job = queue(&app, token, "parameters", 10).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    worker::run_once(&state).await.unwrap();
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    let old = app
        .database
        .collection::<Document>("data_export_attempts")
        .find_one(doc! {"jobId":id})
        .await
        .unwrap()
        .unwrap();
    let store = BlobStore::from_profile(
        &app.database,
        state.config.clone(),
        old.get_object_id("storageProfileId").unwrap(),
    )
    .await
    .unwrap();
    store
        .delete(
            BlobId::parse(&old.get_object_id("blobId").unwrap().to_hex()).unwrap(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_export_attempts")
        .update_one(
            doc! {"_id":old.get_object_id("_id").unwrap()},
            doc! {"$set":{"blobKnown":false,"state":"putting"}},
        )
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
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, &id.to_hex()).await["state"], "unknown");
    assert_eq!(
        app.database
            .collection::<Document>("data_export_attempts")
            .count_documents(doc! {"jobId":id})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        retry(&app, token, &id.to_hex(), &uuid::Uuid::new_v4().to_string())
            .await
            .0,
        StatusCode::ACCEPTED
    );
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, &id.to_hex()).await["state"], "queued");
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, &id.to_hex()).await["state"], "completed");
    let retained = app
        .database
        .collection::<Document>("data_export_attempts")
        .find_one(doc! {"_id":old.get_object_id("_id").unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_ne!(retained.get_str("state").unwrap(), "cleaned");
    assert!(retained.get_bool("writeStarted").unwrap());
    assert_eq!(
        app.database
            .collection::<Document>("data_export_attempts")
            .count_documents(doc! {"jobId":id})
            .await
            .unwrap(),
        2
    );
    app.cleanup().await;
}
#[tokio::test]
async fn retrying_a_stored_user_result_with_narrowed_scope_generates_a_new_authorized_result() {
    let (app, state, _root, auth) = fixture().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let role=app.send("POST","/api/v1/roles",json!({"name":"Retry operator","slug":"retry-operator","permissions":["exports.read","exports.write","users.read"],"dataScope":"all"}),Some(owner)).await.1["data"].clone();
    let role_id = role["id"].as_str().unwrap();
    for name in ["retry.operator", "retry.subject"] {
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/users",
                json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role_id]}),
                Some(owner)
            )
            .await
            .0,
            StatusCode::CREATED
        );
    }
    let login = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"retry.operator","password":PASSWORD}),
            None,
        )
        .await
        .1["data"]
        .clone();
    let token = login["accessToken"].as_str().unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.complete"}},"validationLevel":"strict"}).await.unwrap();
    let job = queue(&app, token, "users", 10).await;
    let id = job["id"].as_str().unwrap();
    worker::run_once(&state).await.unwrap();
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    app.send(
        "PATCH",
        &format!("/api/v1/roles/{role_id}"),
        json!({"version":1,"dataScope":"self"}),
        Some(owner),
    )
    .await;
    assert_eq!(
        retry(&app, token, id, &uuid::Uuid::new_v4().to_string())
            .await
            .0,
        StatusCode::ACCEPTED
    );
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, id).await["state"], "queued");
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, id).await["rowCount"], 1);
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
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("retry.operator"));
    assert!(!text.contains("retry.subject"));
    app.cleanup().await;
}

#[tokio::test]
async fn retry_audit_failure_rolls_back_intent_and_receipts_and_other_owners_cannot_query_actions()
{
    let (app, state, _root, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameter(&app, token, "retry.one").await;
    parameter(&app, token, "retry.two").await;
    let job = queue(&app, token, "parameters", 1).await;
    let id = job["id"].as_str().unwrap();
    worker::run_once(&state).await.unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.retry"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        retry(&app, token, id, &request).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(read(&app, token, id).await["state"], "failed");
    assert_eq!(
        app.database
            .collection::<Document>("data_export_actions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    assert_eq!(
        retry(&app, token, id, &request).await.0,
        StatusCode::ACCEPTED
    );
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"retry-other-owner","displayName":"Other","password":PASSWORD,"roleIds":auth["user"]["roleIds"]}),Some(token)).await.0,StatusCode::CREATED);
    let other = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"retry-other-owner","password":PASSWORD}),
            None,
        )
        .await
        .1["data"]["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/data-tools/exports/{id}/actions/{request}"),
            Value::Null,
            Some(&other)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/data-tools/exports/{id}/actions/{request}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    app.cleanup().await;
}

#[tokio::test]
async fn own_v5_retry_upgrade_preserves_candidate_uncertainty_and_is_repeatable() {
    let (app, state, _root, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameter(&app, token, "retry.one").await;
    parameter(&app, token, "retry.two").await;
    let job = queue(&app, token, "parameters", 1).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    worker::run_once(&state).await.unwrap();
    let before = app
        .database
        .collection::<Document>("data_export_attempts")
        .find_one(doc! {"jobId":id})
        .await
        .unwrap()
        .unwrap();
    app.database
        .collection::<Document>("data_export_attempts")
        .update_one(doc! {"jobId":id}, doc! {"$unset":{"retryRequested":""}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_export_actions")
        .drop()
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"data-tools"}, doc! {"$set":{"version":5}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("data_export_attempts")
            .find_one(doc! {"jobId":id})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(read(&app, token, &id.to_hex()).await["state"], "failed");
    app.cleanup().await;
}

#[tokio::test]
async fn failed_jobs_with_written_candidates_are_confirmed_before_a_new_generation_is_allowed() {
    let (app, state, _root, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    parameter(&app, token, "retry.one").await;
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.complete"}},"validationLevel":"strict"}).await.unwrap();
    let job = queue(&app, token, "parameters", 10).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    worker::run_once(&state).await.unwrap();
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    let candidate = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("candidateId")
        .unwrap();
    app.database
        .collection::<Document>("data_exports")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"state":"failed","lastError":"PERMISSION_REVOKED"}},
        )
        .await
        .unwrap();
    assert_eq!(
        retry(&app, token, &id.to_hex(), &uuid::Uuid::new_v4().to_string())
            .await
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(read(&app, token, &id.to_hex()).await["state"], "unknown");
    worker::run_once(&state).await.unwrap();
    assert_eq!(read(&app, token, &id.to_hex()).await["state"], "completed");
    assert_eq!(
        app.database
            .collection::<Document>("data_exports")
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap()
            .get_object_id("candidateId")
            .unwrap(),
        candidate
    );
    assert_eq!(
        app.database
            .collection::<Document>("data_export_attempts")
            .count_documents(doc! {"jobId":id})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
