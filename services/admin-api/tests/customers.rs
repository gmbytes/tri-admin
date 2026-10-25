mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

async fn role(
    app: &TestApp,
    token: &str,
    slug: &str,
    codes: Value,
    scope: &str,
    ids: Value,
) -> Value {
    let (status,body)=app.send("POST","/api/v1/roles",json!({"name":slug,"slug":slug,"permissions":codes,"dataScope":scope,"dataScopeRoleIds":ids}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}
async fn user(app: &TestApp, token: &str, name: &str, ids: Value) -> Value {
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/users",
            json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":ids}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}
async fn login(app: &TestApp, name: &str) -> Value {
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}
async fn customer(app: &TestApp, token: &str, code: &str, owner: Option<&str>) -> Value {
    let mut body = json!({"code":code,"name":code,"contactName":"联系人","phone":"+86 123456","email":"contact@example.test","note":"测试备注"});
    if let Some(owner) = owner {
        body["ownerId"] = json!(owner);
    }
    let (status, body) = app
        .send("POST", "/api/v1/customers", body, Some(token))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}

#[tokio::test]
async fn customers_have_versioned_crud_readable_owners_and_case_insensitive_active_codes() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let record = customer(&app, token, "Cust-001", None).await;
    assert_eq!(record["ownerId"], auth["user"]["id"]);
    assert_eq!(record["owner"]["displayName"], "管理员");
    assert!(record["createdAt"].as_str().unwrap().ends_with('Z'));
    assert!(
        record.get("_id").is_none()
            && record.get("codeKey").is_none()
            && record.get("nameKey").is_none()
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/customers",
            json!({"code":"cust-001","name":"duplicate"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let path = format!("/api/v1/customers/{}", record["id"].as_str().unwrap());
    let (status, updated) = app
        .send(
            "PATCH",
            &path,
            json!({"version":1,"name":"客户已更新","status":"archived","note":""}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["version"], 2);
    assert_eq!(updated["data"]["code"], "Cust-001");
    assert_eq!(updated["data"]["phone"], "+86 123456");
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"name":"stale"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{path}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{path}?version=2"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(token)).await.0,
        StatusCode::NOT_FOUND
    );
    customer(&app, token, "CUST-001", None).await;
    let audit = app
        .database
        .collection::<Document>("audit_events")
        .find_one(doc! {"action":"customers.update"})
        .await
        .unwrap()
        .unwrap();
    assert!(!audit.contains_key("phone") && !audit.contains_key("note"));
    app.cleanup().await;
}

#[tokio::test]
async fn customer_validation_rejects_null_unknown_fields_invalid_contacts_and_query_fields() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    for body in [
        json!({"code":"","name":"x"}),
        json!({"code":"bad code","name":"x"}),
        json!({"code":"valid","name":""}),
        json!({"code":"valid","name":"x","email":"invalid"}),
        json!({"code":"valid","name":"x","note":"x".repeat(2001)}),
        json!({"code":"valid","name":"x","ownerId":null}),
        json!({"code":"valid","name":"x","status":"unknown"}),
        json!({"code":"valid","name":"x","extra":1}),
    ] {
        assert_eq!(
            app.send("POST", "/api/v1/customers", body, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let record = customer(&app, token, "valid", None).await;
    let path = format!("/api/v1/customers/{}", record["id"].as_str().unwrap());
    for body in [
        json!({"version":1,"note":null}),
        json!({"version":1,"ownerId":null}),
        json!({"version":0,"name":"x"}),
        json!({"version":1,"contactName":"x".repeat(81)}),
    ] {
        assert_eq!(
            app.send("PATCH", &path, body, Some(token)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for query in [
        "page=0",
        "pageSize=101",
        "sort=ownerId",
        "status=unknown",
        "extra=x",
    ] {
        assert_eq!(
            app.send(
                "GET",
                &format!("/api/v1/customers?{query}"),
                Value::Null,
                Some(token)
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
async fn customer_lists_use_literal_search_status_filters_and_stable_pages() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let first = customer(&app, token, "a.one", None).await;
    customer(&app, token, "abone", None).await;
    let path = format!("/api/v1/customers/{}", first["id"].as_str().unwrap());
    app.send(
        "PATCH",
        &path,
        json!({"version":1,"name":"同名客户","status":"archived"}),
        Some(token),
    )
    .await;
    let (_, literal) = app
        .send(
            "GET",
            "/api/v1/customers?search=a.&sort=code",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(literal["meta"]["total"], 1);
    assert_eq!(literal["data"][0]["code"], "a.one");
    let (_, named) = app
        .send(
            "GET",
            "/api/v1/customers?search=%E5%90%8C%E5%90%8D&status=archived",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(named["meta"]["total"], 1);
    let (_, active) = app
        .send(
            "GET",
            "/api/v1/customers?status=active",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(active["meta"]["total"], 1);
    let (_, one) = app
        .send(
            "GET",
            "/api/v1/customers?pageSize=1&sort=createdAt",
            Value::Null,
            Some(token),
        )
        .await;
    let (_, two) = app
        .send(
            "GET",
            "/api/v1/customers?pageSize=1&page=2&sort=createdAt",
            Value::Null,
            Some(token),
        )
        .await;
    assert_ne!(one["data"][0]["id"], two["data"][0]["id"]);
    let (_, empty) = app
        .send("GET", "/api/v1/customers?page=20", Value::Null, Some(token))
        .await;
    assert_eq!(empty["data"], json!([]));
    app.cleanup().await;
}

#[tokio::test]
async fn customer_scope_applies_before_pagination_and_hides_forbidden_records_from_writes() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let codes = json!(["customers.read", "customers.write"]);
    let a = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let b = role(&app, token, "team-b", json!([]), "self", json!([])).await;
    let scoped = role(
        &app,
        token,
        "operator",
        codes.clone(),
        "roles",
        json!([a["id"]]),
    )
    .await;
    let own_role = role(&app, token, "self-reader", codes, "self", json!([])).await;
    let ua = user(&app, token, "member-a", json!([a["id"]])).await;
    let ub = user(&app, token, "member-b", json!([b["id"]])).await;
    user(&app, token, "operator", json!([scoped["id"]])).await;
    user(&app, token, "self-reader", json!([own_role["id"]])).await;
    let inside = customer(&app, token, "INSIDE", ua["id"].as_str()).await;
    let outside = customer(&app, token, "OUTSIDE", ub["id"].as_str()).await;
    let session = login(&app, "operator").await;
    let access = session["accessToken"].as_str().unwrap();
    customer(&app, access, "OWN", None).await;
    let (_, result) = app
        .send(
            "GET",
            "/api/v1/customers?pageSize=1&sort=code",
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(result["meta"]["total"], 2);
    assert_eq!(result["data"].as_array().unwrap().len(), 1);
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/customers/{}", inside["id"].as_str().unwrap()),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    let outside_path = format!("/api/v1/customers/{}", outside["id"].as_str().unwrap());
    for (method, path, body) in [
        ("GET", outside_path.clone(), Value::Null),
        (
            "PATCH",
            outside_path.clone(),
            json!({"version":1,"name":"escape"}),
        ),
        ("DELETE", format!("{outside_path}?version=1"), Value::Null),
    ] {
        assert_eq!(
            app.send(method, &path, body, Some(access)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/customers",
            json!({"code":"ESCAPE","name":"x","ownerId":ub["id"]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/customers/{}", inside["id"].as_str().unwrap()),
            json!({"version":1,"ownerId":ub["id"]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let own = login(&app, "self-reader").await;
    let own_access = own["accessToken"].as_str().unwrap();
    customer(&app, own_access, "SELF", None).await;
    let (_, result) = app
        .send("GET", "/api/v1/customers", Value::Null, Some(own_access))
        .await;
    assert_eq!(result["meta"]["total"], 1);
    assert_eq!(result["data"][0]["code"], "SELF");
    assert_eq!(
        app.send("GET", "/api/v1/customers", Value::Null, Some(token))
            .await
            .1["meta"]["total"],
        4
    );
    app.cleanup().await;
}

#[tokio::test]
async fn customer_owner_choices_are_minimal_scoped_and_need_no_user_read_permission() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let a = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let b = role(&app, token, "team-b", json!([]), "self", json!([])).await;
    let scoped = role(
        &app,
        token,
        "writer",
        json!(["customers.write"]),
        "roles",
        json!([a["id"]]),
    )
    .await;
    let reader_role = role(
        &app,
        token,
        "reader",
        json!(["customers.read"]),
        "all",
        json!([]),
    )
    .await;
    user(&app, token, "member-a", json!([a["id"]])).await;
    user(&app, token, "member-b", json!([b["id"]])).await;
    user(&app, token, "writer", json!([scoped["id"]])).await;
    user(&app, token, "reader", json!([reader_role["id"]])).await;
    let login = login(&app, "writer").await;
    let access = login["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/users", Value::Null, Some(access))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, result) = app
        .send(
            "GET",
            "/api/v1/customers/owners?sort=username",
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["meta"]["total"], 2);
    assert!(
        result["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("roleIds").is_none() && row.get("permissions").is_none())
    );
    let (_, filtered) = app
        .send(
            "GET",
            "/api/v1/customers/owners?search=member-&pageSize=1",
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(filtered["meta"]["total"], 1);
    assert_eq!(filtered["data"][0]["username"], "member-a");
    assert_eq!(
        app.send("GET", "/api/v1/customers", Value::Null, Some(access))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let reader = crate::login(&app, "reader").await;
    let access = reader["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/customers/owners", Value::Null, Some(access))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send("POST", "/api/v1/customers", json!({}), Some(access))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send("GET", "/api/v1/customers", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn customer_transfers_protect_user_deletion_and_only_allow_active_new_owners() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let basic = role(&app, token, "basic", json!([]), "self", json!([])).await;
    let a = user(&app, token, "owner-a", json!([basic["id"]])).await;
    let b = user(&app, token, "owner-b", json!([basic["id"]])).await;
    let record = customer(&app, token, "OWNERSHIP", a["id"].as_str()).await;
    let a_path = format!("/api/v1/users/{}", a["id"].as_str().unwrap());
    let b_path = format!("/api/v1/users/{}", b["id"].as_str().unwrap());
    let path = format!("/api/v1/customers/{}", record["id"].as_str().unwrap());
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{a_path}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"ownerId":mongodb::bson::oid::ObjectId::new().to_hex()}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.send(
            "PATCH",
            &b_path,
            json!({"version":1,"status":"disabled"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"ownerId":b["id"]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.send(
        "PATCH",
        &b_path,
        json!({"version":2,"status":"active"}),
        Some(token),
    )
    .await;
    let (status, updated) = app
        .send(
            "PATCH",
            &path,
            json!({"version":1,"ownerId":b["id"]}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["owner"]["username"], "owner-b");
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{a_path}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{b_path}?version=3"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    app.send(
        "DELETE",
        &format!("{path}?version=2"),
        Value::Null,
        Some(token),
    )
    .await;
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{b_path}?version=3"),
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
async fn existing_customers_of_disabled_owners_can_be_edited_without_reassigning_them() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let basic = role(&app, token, "basic", json!([]), "self", json!([])).await;
    let owner = user(&app, token, "disabled-owner", json!([basic["id"]])).await;
    let record = customer(&app, token, "KEEP-OWNER", owner["id"].as_str()).await;
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{}", owner["id"].as_str().unwrap()),
            json!({"version":1,"status":"disabled"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, result) = app
        .send(
            "PATCH",
            &format!("/api/v1/customers/{}", record["id"].as_str().unwrap()),
            json!({"version":1,"name":"资料更新","ownerId":owner["id"]}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["data"]["ownerId"], owner["id"]);
    app.cleanup().await;
}

#[tokio::test]
async fn populated_customer_lists_have_indexed_pages_and_scope_filtering_stays_on_the_server() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let a = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let b = role(
        &app,
        token,
        "team-b",
        json!(["customers.read"]),
        "self",
        json!([]),
    )
    .await;
    let scoped = role(
        &app,
        token,
        "benchmark-scope",
        json!(["customers.read"]),
        "roles",
        json!([a["id"]]),
    )
    .await;
    let ua = user(&app, token, "bench-a", json!([a["id"]])).await;
    let ub = user(&app, token, "bench-b", json!([b["id"]])).await;
    let operator = user(&app, token, "bench-operator", json!([scoped["id"]])).await;
    let ids = [&auth["user"]["id"], &ua["id"], &ub["id"], &operator["id"]]
        .map(|id| mongodb::bson::oid::ObjectId::parse_str(id.as_str().unwrap()).unwrap());
    let now = mongodb::bson::DateTime::now();
    let records=(0..2000).map(|index|doc!{"code":format!("Q{index:04}"),"codeKey":format!("Q{index:04}"),"name":"客户","nameKey":"客户","contactName":"","phone":"","email":"","note":"","status":"active","ownerId":ids[index%4],"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("customers")
        .insert_many(records)
        .await
        .unwrap();
    for filter in [
        doc! {"deletedAt":null},
        doc! {"deletedAt":null,"ownerId":ids[2]},
    ] {
        let explain=app.database.run_command(doc!{"explain":{"find":"customers","filter":filter,"sort":{"createdAt":-1,"_id":-1},"limit":20},"verbosity":"executionStats"}).await.unwrap();
        let stats = explain.get_document("executionStats").unwrap();
        let number = |key| {
            stats
                .get_i32(key)
                .map(i64::from)
                .or_else(|_| stats.get_i64(key))
                .unwrap()
        };
        assert_eq!(number("nReturned"), 20);
        assert!(number("totalDocsExamined") <= 20, "{stats:?}");
        assert!(number("totalKeysExamined") <= 25, "{stats:?}");
        println!(
            "customers page: returned={},docs={},keys={} of2000",
            number("nReturned"),
            number("totalDocsExamined"),
            number("totalKeysExamined")
        );
    }
    let session = login(&app, "bench-operator").await;
    let access = session["accessToken"].as_str().unwrap();
    let started = std::time::Instant::now();
    let (status, result) = app
        .send(
            "GET",
            "/api/v1/customers?pageSize=20",
            Value::Null,
            Some(access),
        )
        .await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["meta"]["total"], 1000);
    assert_eq!(result["data"].as_array().unwrap().len(), 20);
    assert!(
        result["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["ownerId"] == ua["id"] || row["ownerId"] == operator["id"])
    );
    println!(
        "customers role-scope actual API: input2000,total1000,page20,elapsed={}ms",
        elapsed.as_millis()
    );
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "local role-scope query exceeded3s: {elapsed:?}"
    );
    let session = login(&app, "bench-b").await;
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/customers",
            Value::Null,
            session["accessToken"].as_str()
        )
        .await
        .1["meta"]["total"],
        500
    );
    app.cleanup().await;
}

#[tokio::test]
async fn customer_scope_follows_current_owner_memberships_and_scope_changes_on_the_same_session() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let a = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let b = role(&app, token, "team-b", json!([]), "self", json!([])).await;
    let scope = role(
        &app,
        token,
        "operator",
        json!(["customers.read"]),
        "roles",
        json!([a["id"]]),
    )
    .await;
    let member = user(&app, token, "member", json!([a["id"]])).await;
    user(&app, token, "operator", json!([scope["id"]])).await;
    let record = customer(&app, token, "DYNAMIC", member["id"].as_str()).await;
    let session = login(&app, "operator").await;
    let access = session["accessToken"].as_str().unwrap();
    let path = format!("/api/v1/customers/{}", record["id"].as_str().unwrap());
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(access)).await.0,
        StatusCode::OK
    );
    app.send(
        "PATCH",
        &format!("/api/v1/users/{}", member["id"].as_str().unwrap()),
        json!({"version":1,"roleIds":[b["id"]]}),
        Some(token),
    )
    .await;
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(access)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send("GET", "/api/v1/customers", Value::Null, Some(access))
            .await
            .1["meta"]["total"],
        0
    );
    app.send(
        "PATCH",
        &format!("/api/v1/roles/{}", scope["id"].as_str().unwrap()),
        json!({"version":1,"dataScopeRoleIds":[b["id"]]}),
        Some(token),
    )
    .await;
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(access)).await.0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn customer_creation_and_owner_deletion_cannot_leave_a_live_orphan() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let basic = role(&app, token, "basic", json!([]), "self", json!([])).await;
    let owner = user(&app, token, "race-owner", json!([basic["id"]])).await;
    let path = format!("/api/v1/users/{}?version=1", owner["id"].as_str().unwrap());
    let (delete, create) = tokio::join!(
        app.send("DELETE", &path, Value::Null, Some(token)),
        app.send(
            "POST",
            "/api/v1/customers",
            json!({"code":"RACE","name":"race","ownerId":owner["id"]}),
            Some(token)
        )
    );
    assert!(
        matches!(
            (delete.0, create.0),
            (StatusCode::NO_CONTENT, StatusCode::UNPROCESSABLE_ENTITY)
                | (StatusCode::CONFLICT, StatusCode::CREATED)
        ),
        "delete={delete:?}, create={create:?}"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_customer_updates_and_case_insensitive_creates_have_one_winner() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (one, two) = tokio::join!(
        app.send(
            "POST",
            "/api/v1/customers",
            json!({"code":"RACE","name":"one"}),
            Some(token)
        ),
        app.send(
            "POST",
            "/api/v1/customers",
            json!({"code":"race","name":"two"}),
            Some(token)
        )
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&s| s == StatusCode::CREATED)
            .count(),
        1
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    let record = if one.0 == StatusCode::CREATED {
        one.1
    } else {
        two.1
    };
    let path = format!(
        "/api/v1/customers/{}",
        record["data"]["id"].as_str().unwrap()
    );
    let (one, two) = tokio::join!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"name":"one"}),
            Some(token)
        ),
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"name":"two"}),
            Some(token)
        )
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&s| s == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn customer_schema_is_required_and_unknown_versions_are_rejected() {
    let app = TestApp::new().await;
    let metadata = app.database.collection::<Document>("_schema_migrations");
    assert_eq!(
        metadata
            .count_documents(doc! {"_id":"customers","version":1})
            .await
            .unwrap(),
        1
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    metadata
        .update_one(doc! {"_id":"customers"}, doc! {"$set":{"version":999}})
        .await
        .unwrap();
    assert!(
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .is_err()
    );
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}
