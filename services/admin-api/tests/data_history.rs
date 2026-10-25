mod support;
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::TestApp;
async fn create(app: &TestApp, access: &str, key: &str) -> (Value, Value) {
    let input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"csv","contentBase64":STANDARD.encode(format!("name,key,valueType,value\nPrivate,{key},string,PRIVATE_SOURCE_SENTINEL\n"))});
    let result = app
        .send(
            "POST",
            "/api/v1/data-tools/imports",
            input.clone(),
            Some(access),
        )
        .await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    (input, result.1["data"].clone())
}
#[tokio::test]
async fn history_pages_and_status_filters_do_not_hide_older_pending_jobs() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, old) = create(&app, access, "history.old").await;
    for i in 0..22 {
        let (_, job) = create(&app, access, &format!("history.completed.{i}")).await;
        assert_eq!(
            app.send(
                "POST",
                &format!(
                    "/api/v1/data-tools/imports/{}/apply",
                    job["id"].as_str().unwrap()
                ),
                json!({"rows":[0]}),
                Some(access)
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let page = app
        .send(
            "GET",
            "/api/v1/data-tools/imports?page=2&pageSize=20",
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(page.0, StatusCode::OK, "{}", page.1);
    assert_eq!(page.1["meta"]["total"], 23);
    assert_eq!(page.1["data"].as_array().unwrap().len(), 3);
    let pending = app
        .send(
            "GET",
            "/api/v1/data-tools/imports?status=pending",
            Value::Null,
            Some(access),
        )
        .await
        .1;
    assert_eq!(pending["meta"]["total"], 1);
    assert_eq!(pending["data"][0]["id"], old["id"]);
    assert_eq!(pending["data"][0]["readyCount"], 1);
    assert!(pending["data"][0].get("rows").is_none());
    assert!(!pending.to_string().contains("PRIVATE_SOURCE_SENTINEL"));
    for query in [
        "page=0",
        "pageSize=101",
        "status=arbitrary",
        "resource=unregistered",
        "sql=expression",
    ] {
        assert_eq!(
            app.send(
                "GET",
                &format!("/api/v1/data-tools/imports?{query}"),
                Value::Null,
                Some(access)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    app.cleanup().await;
}
#[tokio::test]
async fn terminal_previews_release_quota_but_thirty_two_active_previews_are_bounded() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let mut first = None;
    for i in 0..32 {
        let (input, job) = create(&app, access, &format!("quota.active.{i}")).await;
        if i == 0 {
            first = Some((input, job));
        }
    }
    let body = json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"csv","contentBase64":STANDARD.encode("name,key,valueType,value\nNext,quota.next,string,x\n")});
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/data-tools/imports",
            body.clone(),
            Some(access)
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let (original, job) = first.unwrap();
    let id = job["id"].as_str().unwrap();
    assert_eq!(
        app.send("POST", "/api/v1/data-tools/imports", original, Some(access))
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/imports/{id}/apply"),
            json!({"rows":[0]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("POST", "/api/v1/data-tools/imports", body, Some(access))
            .await
            .0,
        StatusCode::CREATED
    );
    app.cleanup().await;
}
#[tokio::test]
async fn expired_content_is_unavailable_before_cleanup_while_receipts_remain_queryable() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, job) = create(&app, access, "expiry.private").await;
    let id = job["id"].as_str().unwrap();
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let result = app
        .send(
            "GET",
            &format!("/api/v1/data-tools/imports/{id}"),
            Value::Null,
            Some(access),
        )
        .await
        .1;
    assert_eq!(result["data"]["payloadState"], "expired");
    assert_eq!(result["data"]["status"], "expired");
    assert_eq!(result["data"]["rows"][0]["status"], "expired");
    assert!(result["data"]["rows"][0]["values"].is_null());
    assert!(result["data"]["rows"][0]["key"].is_null());
    assert!(!result.to_string().contains("PRIVATE_SOURCE_SENTINEL"));
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/imports/{id}/apply"),
            json!({"rows":[0]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    app.cleanup().await;
}

#[tokio::test]
async fn own_v2_upgrade_builds_counters_without_resetting_receipts_or_later_redaction() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, job) = create(&app, access, "upgrade.history").await;
    let id = job["id"].as_str().unwrap();
    app.send(
        "POST",
        &format!("/api/v1/data-tools/imports/{id}/apply"),
        json!({"rows":[0]}),
        Some(access),
    )
    .await;
    app.database.collection::<Document>("data_imports").update_one(doc!{"_id":ObjectId::parse_str(id).unwrap()},doc!{"$unset":{"rowCount":"","readyCount":"","appliedCount":"","errorCount":"","expiredCount":"","payloadState":"","redactedAt":""}}).await.unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"data-tools"}, doc! {"$set":{"version":2}})
        .await
        .unwrap();
    assert!(
        !admin_api::storage::schema::is_initialized(&app.database)
            .await
            .unwrap()
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let result = app
        .send(
            "GET",
            &format!("/api/v1/data-tools/imports/{id}"),
            Value::Null,
            Some(access),
        )
        .await
        .1;
    assert_eq!(result["data"]["rowCount"], 1);
    assert_eq!(result["data"]["appliedCount"], 1);
    assert_eq!(result["data"]["readyCount"], 0);
    assert_eq!(result["data"]["status"], "completed");
    let target = result["data"]["rows"][0]["targetId"].clone();
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    admin_api::modules::data_tools::retention::maintenance_once(&app.database)
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let result = app
        .send(
            "POST",
            &format!("/api/v1/data-tools/imports/{id}/apply"),
            json!({"rows":[0]}),
            Some(access),
        )
        .await
        .1;
    assert_eq!(result["data"]["payloadState"], "redacted");
    assert_eq!(result["data"]["rows"][0]["targetId"], target);
    assert!(result["data"]["rows"][0]["values"].is_null());
    app.cleanup().await;
}

#[tokio::test]
async fn terminal_status_filters_remain_consistent_before_and_after_payload_cleanup() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, expired) = create(&app, access, "matrix.expired").await;
    let (_, complete) = create(&app, access, "matrix.complete").await;
    app.send(
        "POST",
        &format!(
            "/api/v1/data-tools/imports/{}/apply",
            complete["id"].as_str().unwrap()
        ),
        json!({"rows":[0]}),
        Some(access),
    )
    .await;
    let failed=app.send("POST","/api/v1/data-tools/imports",json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"csv","contentBase64":STANDARD.encode("name,key,valueType,value\nInvalid,matrix.invalid,boolean,maybe\n")}),Some(access)).await.1["data"].clone();
    let partial=app.send("POST","/api/v1/data-tools/imports",json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"csv","contentBase64":STANDARD.encode("name,key,valueType,value\nOne,matrix.partial.one,string,x\nTwo,matrix.partial.two,string,y\n")}),Some(access)).await.1["data"].clone();
    app.send(
        "POST",
        &format!(
            "/api/v1/data-tools/imports/{}/apply",
            partial["id"].as_str().unwrap()
        ),
        json!({"rows":[0]}),
        Some(access),
    )
    .await;
    for job in [&expired, &partial] {
        app.database
            .collection::<Document>("data_imports")
            .update_one(
                doc! {"_id":ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap()},
                doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
    }
    for pass in 0..2 {
        if pass == 1 {
            admin_api::modules::data_tools::retention::maintenance_once(&app.database)
                .await
                .unwrap();
        }
        for (status, job) in [
            ("completed", &complete),
            ("failed", &failed),
            ("partial", &partial),
            ("expired", &expired),
        ] {
            let result = app
                .send(
                    "GET",
                    &format!("/api/v1/data-tools/imports?status={status}"),
                    Value::Null,
                    Some(access),
                )
                .await;
            assert_eq!(result.0, StatusCode::OK, "{}", result.1);
            assert_eq!(result.1["meta"]["total"], 1);
            assert_eq!(result.1["data"][0]["id"], job["id"]);
            assert_eq!(result.1["data"][0]["status"], status);
        }
        assert_eq!(
            app.send(
                "GET",
                "/api/v1/data-tools/imports?status=pending",
                Value::Null,
                Some(access)
            )
            .await
            .1["meta"]["total"],
            0
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn private_import_results_explicitly_disable_http_caching() {
    use axum::{
        body::Body,
        http::{Request, header::CACHE_CONTROL},
    };
    use tower::ServiceExt;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, job) = create(&app, access, "cache.private").await;
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/data-tools/imports/{}",
                    job["id"].as_str().unwrap()
                ))
                .header("authorization", format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("private, no-store")
    );
    app.cleanup().await;
}
