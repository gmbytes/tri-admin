mod support;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
use tower::ServiceExt;

async fn entries(app: &TestApp, token: &str, query: &str) -> Value {
    let (status, body) = app
        .send(
            "GET",
            &format!("/api/v1/logs?{query}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}
async fn identity(app: &TestApp, owner: &str, codes: Value) -> String {
    let (_, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":"log-viewer","slug":"log-viewer","permissions":codes,"dataScope":"self"}),
            Some(owner),
        )
        .await;
    let(status,result)=app.send("POST","/api/v1/users",json!({"username":"log-viewer","displayName":"Log viewer","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    let (_, body) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"log-viewer","password":PASSWORD}),
            None,
        )
        .await;
    body["data"]["accessToken"].as_str().unwrap().into()
}
fn past(days: i64) -> DateTime {
    DateTime::from_millis(DateTime::now().timestamp_millis() - days * 86400000)
}
async fn fixture(app: &TestApp, kind: &str, number: usize) -> Vec<ObjectId> {
    let ids = (0..number).map(|_| ObjectId::new()).collect::<Vec<_>>();
    let collection = if kind == "error" {
        "request_errors"
    } else {
        "audit_events"
    };
    app.database.collection::<Document>(collection).insert_many(ids.iter().map(|id|doc!{"_id":id,"kind":kind,"category":"administration","action":"fixture.old","outcome":"success","createdAt":past(3)})).await.unwrap();
    ids
}

#[tokio::test]
async fn log_lists_are_permission_gated_bounded_and_do_not_return_unknown_stored_fields() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let ids = fixture(&app, "operation", 3).await;
    app.database.collection::<Document>("audit_events").update_one(doc!{"_id":ids[0]},doc!{"$set":{"password":"never-return-this","body":"never-return-this","response":"never-return-this"}}).await.unwrap();
    let list = entries(
        &app,
        owner,
        "kind=operation&action=fixture.old&pageSize=1&sort=createdAt",
    )
    .await;
    assert_eq!(list["meta"]["total"], 3);
    assert_eq!(list["data"][0]["id"], ids[0].to_hex());
    assert!(!list.to_string().contains("never-return-this"));
    let next = entries(
        &app,
        owner,
        "kind=operation&action=fixture.old&pageSize=1&page=2&sort=createdAt",
    )
    .await;
    assert_eq!(next["data"][0]["id"], ids[1].to_hex());
    assert_eq!(
        app.send("GET", "/api/v1/logs", Value::Null, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let denied = identity(&app, owner, json!([])).await;
    assert_eq!(
        app.send("GET", "/api/v1/logs", Value::Null, Some(&denied))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for query in [
        "kind=unknown",
        "pageSize=101",
        "page=0",
        "sort=password",
        "unknown=value",
        "from=bad",
        "from=2026-01-02T00:00:00Z&to=2026-01-01T00:00:00Z",
        "actorId=bad",
        "requestId=bad",
        "kind=operation&status=999",
        "kind=operation&username=admin",
    ] {
        assert_eq!(
            app.send(
                "GET",
                &format!("/api/v1/logs?{query}"),
                Value::Null,
                Some(owner)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{query}"
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn login_failures_rate_blocks_and_successes_are_recorded_without_credentials() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let wrong = "DO-NOT-LOG-password-or-token";
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"absent-user","password":wrong}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..11 {
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"rate-fixture","password":wrong}),
            None,
        )
        .await;
    }
    let success = entries(&app, owner, "kind=login&username=admin&outcome=success").await;
    assert_eq!(success["meta"]["total"], 1);
    let failed = entries(
        &app,
        owner,
        "kind=login&username=absent-user&outcome=failure",
    )
    .await;
    assert_eq!(failed["meta"]["total"], 1);
    let blocked = entries(
        &app,
        owner,
        "kind=login&username=rate-fixture&outcome=blocked",
    )
    .await;
    assert_eq!(blocked["meta"]["total"], 1);
    assert_eq!(blocked["data"][0]["errorCode"], "RATE_LIMITED");
    use futures_util::TryStreamExt;
    let stored = app
        .database
        .collection::<Document>("audit_events")
        .find(doc! {"kind":"login"})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert!(!format!("{stored:?}").contains(wrong));
    app.cleanup().await;
}

#[tokio::test]
async fn errors_use_safe_route_templates_request_ids_and_actor_attribution() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/file-categories?password=secret-query")
                .header("authorization", format!("Bearer {owner}"))
                .header("content-type", "application/json")
                .header("x-request-id", "secret-untrusted-header")
                .body(Body::from(r#"{"secret":"secret-body"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    let id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    let logs = entries(&app, owner, &format!("kind=error&requestId={id}")).await;
    assert_eq!(logs["meta"]["total"], 1);
    let row = &logs["data"][0];
    assert_eq!(row["route"], "/api/v1/file-categories");
    assert_eq!(row["action"], "fileCategories.create");
    assert_eq!(row["errorCode"], "VALIDATION_ERROR");
    assert_eq!(row["actorId"], auth["user"]["id"]);
    assert!(row["durationMs"].as_i64().is_some());
    let request_id = uuid::Uuid::new_v4().to_string();
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/unknown/secret-path?token=secret-query")
                .header("x-request-id", &request_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["x-request-id"], request_id.as_str());
    let unknown = entries(&app, owner, &format!("kind=error&requestId={request_id}")).await;
    assert_eq!(unknown["data"][0]["route"], "<unmatched>");
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/users/%FF")
                .header("authorization", format!("Bearer {owner}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.headers()["content-type"], "application/json");
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["error"]["code"], "BAD_REQUEST");
    use futures_util::TryStreamExt;
    let raw = app
        .database
        .collection::<Document>("request_errors")
        .find(doc! {})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let raw = format!("{raw:?}");
    for secret in [
        "secret-query",
        "secret-body",
        "secret-path",
        "secret-untrusted-header",
        owner,
    ] {
        assert!(!raw.contains(secret));
    }
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_operations_keep_their_own_request_id_and_completion_measurements() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let one = uuid::Uuid::new_v4().to_string();
    let two = uuid::Uuid::new_v4().to_string();
    let request = |key: &str, id: &str| {
        Request::builder()
            .method("POST")
            .uri("/api/v1/file-categories")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .header("x-request-id", id)
            .body(Body::from(
                json!({"name":key,"code":key,"description":"private-value"}).to_string(),
            ))
            .unwrap()
    };
    let (a, b) = tokio::join!(
        app.router.clone().oneshot(request("request-one", &one)),
        app.router.clone().oneshot(request("request-two", &two))
    );
    assert_eq!(a.unwrap().status(), 201);
    assert_eq!(b.unwrap().status(), 201);
    let shared_id = one.clone();
    for id in [one, two] {
        let list = entries(&app, token, &format!("kind=operation&requestId={id}")).await;
        assert_eq!(list["meta"]["total"], 1);
        assert_eq!(list["data"][0]["requestId"], id);
        assert_eq!(list["data"][0]["status"], 201);
        assert!(list["data"][0]["durationMs"].is_number());
        assert!(!list.to_string().contains("private-value"));
    }
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/file-categories")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .header("x-request-id", &shared_id)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let list = entries(
        &app,
        token,
        &format!("kind=operation&requestId={shared_id}"),
    )
    .await;
    assert_eq!(list["meta"]["total"], 1);
    assert_eq!(
        list["data"][0]["status"], 201,
        "another request sharing an external id cannot rewrite this event"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn bounded_cleanup_is_atomic_idempotent_and_cannot_remove_recent_or_other_kind_events() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let ids = fixture(&app, "operation", 3).await;
    fixture(&app, "error", 1).await;
    let input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"kind":"operation","before":chrono::DateTime::from_timestamp_millis(past(2).timestamp_millis()).unwrap().to_rfc3339(),"limit":2});
    let (a, b) = tokio::join!(
        app.send("POST", "/api/v1/logs/cleanup", input.clone(), Some(token)),
        app.send("POST", "/api/v1/logs/cleanup", input.clone(), Some(token))
    );
    assert_eq!(a.0, StatusCode::OK, "{:?}", a.1);
    assert_eq!(a, b);
    assert_eq!(a.1["data"]["deletedCount"], 2);
    assert_eq!(a.1["data"]["hasMore"], true);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"_id":{"$in":ids}})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("request_errors")
            .count_documents(doc! {"action":"fixture.old"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"logs.cleanup"})
            .await
            .unwrap(),
        1
    );
    let mut conflict = input.clone();
    conflict["limit"] = json!(3);
    assert_eq!(
        app.send("POST", "/api/v1/logs/cleanup", conflict, Some(token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let viewer = identity(&app, token, json!(["audit.read"])).await;
    assert_eq!(
        app.send("POST", "/api/v1/logs/cleanup", input.clone(), Some(&viewer))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"slug":"log-viewer"},
            doc! {"$set":{"permissions":["audit.read","audit.delete"]}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send("POST", "/api/v1/logs/cleanup", input.clone(), Some(&viewer))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    for patch in [
        json!({"limit":0}),
        json!({"limit":1001}),
        json!({"before":"bad"}),
        json!({"before":chrono::Utc::now().to_rfc3339()}),
        json!({"requestId":"bad"}),
    ] {
        let mut bad = input.clone();
        for (k, v) in patch.as_object().unwrap() {
            bad[k] = v.clone();
        }
        assert_eq!(
            app.send("POST", "/api/v1/logs/cleanup", bad, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn cleanup_result_write_failure_rolls_back_deleted_events_and_success_audit() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let ids = fixture(&app, "operation", 2).await;
    app.database.run_command(doc!{"collMod":"log_cleanups","validator":{"$jsonSchema":{"required":["rejectFixture"]}},"validationLevel":"strict"}).await.unwrap();
    let input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"kind":"operation","before":chrono::DateTime::from_timestamp_millis(past(2).timestamp_millis()).unwrap().to_rfc3339()});
    assert_eq!(
        app.send("POST", "/api/v1/logs/cleanup", input, Some(token))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"_id":{"$in":ids}})
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"logs.cleanup"})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn logs_schema_is_required_and_failed_request_log_storage_preserves_the_response() {
    let app = TestApp::new().await;
    assert_eq!(
        app.database
            .collection::<Document>("_schema_migrations")
            .count_documents(doc! {"_id":"logs","version":2})
            .await
            .unwrap(),
        1
    );
    app.database.run_command(doc!{"collMod":"request_errors","validator":{"$jsonSchema":{"required":["rejectFixture"]}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/file-categories", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.database
        .collection::<Document>("_schema_migrations")
        .delete_one(doc! {"_id":"logs"})
        .await
        .unwrap();
    app.database
        .collection::<Document>("request_errors")
        .drop()
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(
        !app.database
            .list_collection_names()
            .await
            .unwrap()
            .iter()
            .any(|name| name == "request_errors")
    );
    app.cleanup().await;
}

#[tokio::test]
async fn session_security_operations_are_available_with_their_request_metadata() {
    let app = TestApp::new().await;
    let owner = app.login().await;
    let token = owner["accessToken"].as_str().unwrap();
    let other = app.login().await;
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/logout",
            json!({"refreshToken":other["refreshToken"]}),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let other = app.login().await;
    let refresh = other["refreshToken"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/refresh",
            json!({"refreshToken":refresh}),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/refresh",
            json!({"refreshToken":refresh}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    for (action, status) in [("auth.logout", 204), ("auth.refreshReuse", 401)] {
        let logs = entries(&app, token, &format!("kind=operation&action={action}")).await;
        assert_eq!(logs["meta"]["total"], 1, "{logs}");
        assert_eq!(logs["data"][0]["status"], status);
        assert!(uuid::Uuid::parse_str(logs["data"][0]["requestId"].as_str().unwrap()).is_ok());
    }
    app.cleanup().await;
}

#[tokio::test]
async fn a_handler_panic_returns_a_sanitized_error_and_is_persisted_without_its_payload() {
    async fn panic_fixture() -> StatusCode {
        panic!("PRIVATE-PANIC-PAYLOAD");
    }
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let config = admin_api::config::Config::from_lookup(|key| {
        (key == "TRI_JWT_SIGNING_KEY").then(|| support::KEY.into())
    })
    .unwrap();
    let state = admin_api::state::AppState::new(config, Some(app.database.clone()));
    let router = axum::Router::new()
        .route("/fixture-panic", axum::routing::get(panic_fixture))
        .layer(axum::middleware::from_fn_with_state(
            state,
            admin_api::common::observability::observe,
        ));
    let response = router
        .oneshot(
            Request::builder()
                .uri("/fixture-panic")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("PRIVATE-PANIC-PAYLOAD"));
    let list = entries(&app, token, &format!("kind=error&requestId={id}")).await;
    assert_eq!(list["meta"]["total"], 1);
    assert_eq!(list["data"][0]["errorCode"], "INTERNAL_ERROR");
    assert!(!list.to_string().contains("PRIVATE-PANIC-PAYLOAD"));
    app.cleanup().await;
}

#[tokio::test]
async fn logs_menu_migration_requires_v2_and_preserves_customization_and_deletion() {
    let app = TestApp::new().await;
    let menus = app.database.collection::<Document>("menus");
    menus.delete_many(doc! {"path":"/logs"}).await.unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"logs"}, doc! {"$set":{"version":1}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let row = menus
        .find_one(doc! {"path":"/logs","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    let id = row.get_object_id("_id").unwrap();
    assert_eq!(row.get_str("viewKey").unwrap(), "audit");
    assert_eq!(row.get_str("permission").unwrap(), "audit.read");
    menus
        .update_one(doc! {"_id":id}, doc! {"$set":{"label":"自定义日志中心"}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        menus
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap()
            .get_str("label")
            .unwrap(),
        "自定义日志中心"
    );
    menus
        .update_one(doc! {"_id":id}, doc! {"$set":{"deletedAt":DateTime::now()}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        menus
            .count_documents(doc! {"path":"/logs","deletedAt":null})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    app.cleanup().await;
}
