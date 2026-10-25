mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
const BASE: &str = "/api/v1/data-tools/exports";
fn input(resource: &str) -> Value {
    let field = if resource == "users" {
        "username"
    } else {
        "key"
    };
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":"csv","maxRows":100000,"source":{"kind":"resource","resource":resource,"columns":[{"field":field,"header":"标识"}],"filters":[],"search":"","sort":null}})
}
async fn create(app: &TestApp, token: &str, body: Value) -> Value {
    let result = app.send("POST", BASE, body, Some(token)).await;
    assert_eq!(result.0, StatusCode::ACCEPTED, "{}", result.1);
    result.1["data"].clone()
}
async fn operator(app: &TestApp, owner: &str, name: &str, permissions: Value) -> (String, String) {
    let role = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":name,"slug":name,"permissions":permissions,"dataScope":"self"}),
            Some(owner),
        )
        .await;
    assert_eq!(role.0, StatusCode::CREATED, "{}", role.1);
    let role_id = role.1["data"]["id"].as_str().unwrap().to_owned();
    let user = app
        .send(
            "POST",
            "/api/v1/users",
            json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role_id]}),
            Some(owner),
        )
        .await;
    assert_eq!(user.0, StatusCode::CREATED, "{}", user.1);
    let login = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(login.0, StatusCode::OK);
    (
        login.1["data"]["accessToken"].as_str().unwrap().into(),
        role_id,
    )
}

#[tokio::test]
async fn creation_replay_lookup_and_cancellation_are_durable_and_owner_only() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let body = input("parameters");
    let (left, right) = tokio::join!(
        app.send("POST", BASE, body.clone(), Some(token)),
        app.send("POST", BASE, body.clone(), Some(token))
    );
    assert!(
        [left.0, right.0].contains(&StatusCode::ACCEPTED),
        "{} {}",
        left.1,
        right.1
    );
    assert!([left.0, right.0].contains(&StatusCode::OK));
    assert_eq!(left.1["data"]["id"], right.1["data"]["id"]);
    let job = &left.1["data"];
    let id = job["id"].as_str().unwrap();
    assert_eq!(job["state"], "queued");
    assert!(job["file"].is_null());
    assert_eq!(job["rowCount"], 0);
    for private in [
        "spec",
        "creationScope",
        "storageProfileId",
        "leaseToken",
        "fingerprint",
        "candidateId",
    ] {
        assert!(job.get(private).is_none());
    }
    for path in [
        format!("{BASE}/{id}"),
        format!("{BASE}/by-request/{}", body["requestId"].as_str().unwrap()),
    ] {
        let result = app.send("GET", &path, Value::Null, Some(token)).await;
        assert_eq!(result.0, StatusCode::OK);
        assert_eq!(result.1["data"]["id"], id);
    }
    let mut different = body.clone();
    different["format"] = json!("xlsx");
    assert_eq!(
        app.send("POST", BASE, different, Some(token)).await.0,
        StatusCode::CONFLICT
    );
    let mut defaults = body.clone();
    defaults.as_object_mut().unwrap().remove("maxRows");
    for field in ["search", "filters", "sort"] {
        defaults["source"].as_object_mut().unwrap().remove(field);
    }
    assert_eq!(
        app.send("POST", BASE, defaults, Some(token)).await.1["data"]["id"],
        id
    );
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"other-owner","displayName":"Other Owner","password":PASSWORD,"roleIds":auth["user"]["roleIds"]}),Some(token)).await.0,StatusCode::CREATED);
    let other = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"other-owner","password":PASSWORD}),
            None,
        )
        .await
        .1["data"]["accessToken"]
        .as_str()
        .unwrap()
        .to_owned();
    for (method, path, value) in [
        ("GET", format!("{BASE}/{id}"), Value::Null),
        (
            "GET",
            format!("{BASE}/by-request/{}", body["requestId"].as_str().unwrap()),
            Value::Null,
        ),
        ("POST", format!("{BASE}/{id}/cancel"), json!({})),
    ] {
        assert_eq!(
            app.send(method, &path, value, Some(&other)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        app.send("GET", BASE, Value::Null, Some(&other)).await.1["meta"]["total"],
        0
    );
    for _ in 0..2 {
        assert_eq!(
            app.send(
                "POST",
                &format!("{BASE}/{id}/cancel"),
                json!({}),
                Some(token)
            )
            .await
            .1["data"]["state"],
            "cancelled"
        );
    }
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.create"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.cancel"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.send("POST", BASE, body, Some(token)).await.1["data"]["state"],
        "cancelled"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn quotas_are_atomic_global_and_per_actor_and_replays_do_not_consume_slots() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let body = input("parameters");
    let first = create(&app, token, body.clone()).await;
    create(&app, token, input("parameters")).await;
    assert_eq!(
        app.send("POST", BASE, input("parameters"), Some(token))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        app.send("POST", BASE, body, Some(token)).await.0,
        StatusCode::OK
    );
    app.send(
        "POST",
        &format!("{BASE}/{}/cancel", first["id"].as_str().unwrap()),
        json!({}),
        Some(token),
    )
    .await;
    create(&app, token, input("parameters")).await;
    for index in 0..3 {
        let (operator, _) = operator(
            &app,
            token,
            &format!("quota-{index}"),
            json!(["exports.read", "exports.write", "parameters.read"]),
        )
        .await;
        create(&app, &operator, input("parameters")).await;
        create(&app, &operator, input("parameters")).await;
    }
    let (extra, _) = operator(
        &app,
        token,
        "quota-extra",
        json!(["exports.read", "exports.write", "parameters.read"]),
    )
    .await;
    assert_eq!(
        app.send("POST", BASE, input("parameters"), Some(&extra))
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        app.database
            .collection::<Document>("data_exports")
            .count_documents(doc! {"state":"queued"})
            .await
            .unwrap(),
        8
    );
    app.cleanup().await;
}

#[tokio::test]
async fn template_and_default_storage_snapshots_survive_configuration_changes_and_replay() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let template=app.send("POST","/api/v1/export-templates",json!({"name":"Snapshot","code":"job.snapshot","resource":"parameters","columns":[{"field":"key","header":"原标识"}],"search":"snapshot.","filters":[{"field":"group","op":"eq","value":"red"}],"sort":{"field":"key","direction":"desc"}}),Some(token)).await;
    assert_eq!(template.0, StatusCode::CREATED);
    let template_id = template.1["data"]["id"].as_str().unwrap();
    let body = json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":"xlsx","maxRows":500000,"source":{"kind":"template","id":template_id,"version":1}});
    let job = create(&app, token, body.clone()).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    let stored = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored
            .get_document("spec")
            .unwrap()
            .get_str("search")
            .unwrap(),
        "snapshot."
    );
    assert_eq!(
        stored
            .get_document("spec")
            .unwrap()
            .get_array("columns")
            .unwrap()[0]
            .as_document()
            .unwrap()
            .get_str("header")
            .unwrap(),
        "原标识"
    );
    assert_eq!(job["template"]["version"], 1);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/export-templates/{template_id}"),
            json!({"version":1,"name":"Changed","search":"other."}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut stale = body.clone();
    stale["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(
        app.send("POST", BASE, stale, Some(token)).await.0,
        StatusCode::CONFLICT
    );
    app.send(
        "DELETE",
        &format!("/api/v1/export-templates/{template_id}?version=2"),
        Value::Null,
        Some(token),
    )
    .await;
    assert_eq!(
        app.send("POST", BASE, body.clone(), Some(token)).await.1["data"]["id"],
        job["id"]
    );
    let replacement=app.send("POST","/api/v1/settings/profiles",json!({"name":"Replacement","code":"replacement-storage","enabled":true,"configuration":{"kind":"localStorage","subdirectory":"replacement"}}),Some(token)).await;
    assert_eq!(replacement.0, StatusCode::CREATED);
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":1,"storageProfileId":replacement.1["data"]["id"]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let old = stored.get_object_id("storageProfileId").unwrap().to_hex();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{old}"),
            json!({"version":1,"enabled":false}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send("POST", BASE, body, Some(token)).await.1["data"]["id"],
        job["id"]
    );
    app.send(
        "POST",
        &format!("{BASE}/{id}/cancel"),
        json!({}),
        Some(token),
    )
    .await;
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{old}"),
            json!({"version":1,"enabled":false}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn current_resource_permissions_filter_pages_and_creation_scope_is_private() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let (token, role) = operator(
        &app,
        owner,
        "export-operator",
        json!([
            "exports.read",
            "exports.write",
            "parameters.read",
            "users.read"
        ]),
    )
    .await;
    let first = create(&app, &token, input("parameters")).await;
    let second = create(&app, &token, input("users")).await;
    let stored = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":ObjectId::parse_str(second["id"].as_str().unwrap()).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored
            .get_document("creationScope")
            .unwrap()
            .get_str("scope")
            .unwrap(),
        "self"
    );
    let page = app
        .send(
            "GET",
            &format!("{BASE}?page=2&pageSize=1"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(page.1["meta"]["total"], 2);
    assert_eq!(page.1["data"].as_array().unwrap().len(), 1);
    app.send(
        "PATCH",
        &format!("/api/v1/roles/{role}"),
        json!({"version":1,"permissions":["exports.read","exports.write","users.read"]}),
        Some(owner),
    )
    .await;
    let page = app.send("GET", BASE, Value::Null, Some(&token)).await;
    assert_eq!(page.1["meta"]["total"], 1);
    assert_eq!(page.1["data"][0]["id"], second["id"]);
    assert_eq!(
        app.send(
            "GET",
            &format!("{BASE}/{}", first["id"].as_str().unwrap()),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("{BASE}?resource=parameters"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (reader, _) = operator(
        &app,
        owner,
        "export-reader",
        json!(["exports.read", "parameters.read"]),
    )
    .await;
    assert_eq!(
        app.send("POST", BASE, input("parameters"), Some(&reader))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn invalid_sources_queries_and_limits_never_create_jobs() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut cases = Vec::new();
    for (key, value) in [
        ("maxRows", json!(0)),
        ("maxRows", json!(1000001)),
        ("requestId", json!("NOT-A-UUID")),
        ("format", json!("sql")),
        ("storageProfileId", json!("../private")),
        ("unknown", json!(true)),
    ] {
        let mut body = input("parameters");
        body[key] = value;
        cases.push(body);
    }
    for source in [
        json!({"kind":"sql","statement":"select * from users"}),
        json!({"kind":"template","id":"bad","version":1}),
        json!({"kind":"resource","resource":"users","columns":[{"field":"password","header":"secret"}]}),
        json!({"kind":"resource","resource":"parameters","columns":[{"field":"key","header":"=FORMULA"}]}),
        json!({"kind":"resource","resource":"parameters","columns":[{"field":"key","header":"key"}],"filters":[{"field":"key","op":"eq","value":{"$ne":null}}]}),
    ] {
        let mut body = input("parameters");
        body["source"] = source;
        cases.push(body);
    }
    for body in cases {
        assert_eq!(
            app.send("POST", BASE, body, Some(token)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for query in [
        "page=0",
        "pageSize=101",
        "state=invalid",
        "resource=customers",
        "sql=private",
    ] {
        assert_eq!(
            app.send("GET", &format!("{BASE}?{query}"), Value::Null, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        app.database
            .collection::<Document>("data_exports")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn audit_failure_rolls_back_creation_and_cancellation() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.create"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send("POST", BASE, input("parameters"), Some(token))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("data_exports")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    let job = create(&app, token, input("parameters")).await;
    let id = job["id"].as_str().unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.cancel"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("{BASE}/{id}/cancel"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.send("GET", &format!("{BASE}/{id}"), Value::Null, Some(token))
            .await
            .1["data"]["state"],
        "queued"
    );
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("{BASE}/{id}/cancel"),
            json!({}),
            Some(token)
        )
        .await
        .1["data"]["state"],
        "cancelled"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn expired_result_metadata_is_hidden_without_deleting_the_receipt() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let job = create(&app, token, input("parameters")).await;
    let id = job["id"].as_str().unwrap();
    app.database.collection::<Document>("data_exports").update_one(doc!{"_id":ObjectId::parse_str(id).unwrap()},doc!{"$set":{"state":"completed","completedAt":DateTime::from_millis(0),"expiresAt":DateTime::from_millis(1),"output":{"blobId":ObjectId::new(),"name":"private.csv","mimeType":"text/csv;charset=utf-8","size":1i64,"sha256":"a".repeat(64)}}}).await.unwrap();
    let result = app
        .send("GET", &format!("{BASE}/{id}"), Value::Null, Some(token))
        .await;
    assert_eq!(result.1["data"]["state"], "expired");
    assert!(result.1["data"]["file"].is_null());
    assert_eq!(result.1["data"]["cleanupPending"], true);
    assert_eq!(
        app.send(
            "GET",
            &format!("{BASE}?state=completed"),
            Value::Null,
            Some(token)
        )
        .await
        .1["meta"]["total"],
        0
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("{BASE}?state=expired"),
            Value::Null,
            Some(token)
        )
        .await
        .1["meta"]["total"],
        1
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("{BASE}/{id}/cancel"),
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
async fn simultaneous_distinct_requests_cannot_overrun_the_actor_quota() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (a, b, c) = tokio::join!(
        app.send("POST", BASE, input("parameters"), Some(token)),
        app.send("POST", BASE, input("parameters"), Some(token)),
        app.send("POST", BASE, input("parameters"), Some(token))
    );
    let statuses = [a.0, b.0, c.0];
    assert_eq!(
        statuses
            .iter()
            .filter(|value| **value == StatusCode::ACCEPTED)
            .count(),
        2
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|value| **value == StatusCode::TOO_MANY_REQUESTS)
            .count(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("data_exports")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    app.cleanup().await;
}

#[tokio::test]
async fn job_metadata_is_private_no_store_and_never_contains_saved_selectors() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut body = input("parameters");
    body["source"]["search"] = json!("PRIVATE_EXPORT_SELECTOR");
    let request = body["requestId"].as_str().unwrap().to_owned();
    let job = create(&app, token, body).await;
    let id = job["id"].as_str().unwrap();
    for path in [
        BASE.to_owned(),
        format!("{BASE}/{id}"),
        format!("{BASE}/by-request/{request}"),
    ] {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("PRIVATE_EXPORT_SELECTOR"));
        assert!(!text.contains("storageProfileId"));
        assert!(!text.contains("leaseToken"));
    }
    app.cleanup().await;
}

#[tokio::test]
async fn own_v4_migration_adds_durable_jobs_and_repeating_it_preserves_receipts() {
    let app = TestApp::new().await;
    for name in [
        "data_exports",
        "data_export_attempts",
        "data_export_manifest",
    ] {
        app.database
            .collection::<Document>(name)
            .drop()
            .await
            .unwrap();
    }
    app.database
        .collection::<Document>("_invariants")
        .delete_one(doc! {"_id":"data-exports"})
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"data-tools"}, doc! {"$set":{"version":4}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("_schema_migrations")
            .find_one(doc! {"_id":"data-tools"})
            .await
            .unwrap()
            .unwrap()
            .get_i32("version")
            .unwrap(),
        5
    );
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let job = create(&app, token, input("parameters")).await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    let before = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("data_exports")
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    app.cleanup().await;
}
