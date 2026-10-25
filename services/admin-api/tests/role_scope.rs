mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

async fn role(
    app: &TestApp,
    token: &str,
    slug: &str,
    permissions: Value,
    scope: &str,
    ids: Value,
) -> Value {
    let (status,body)=app.send("POST","/api/v1/roles",json!({"name":slug,"slug":slug,"permissions":permissions,"dataScope":scope,"dataScopeRoleIds":ids}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}
async fn user(app: &TestApp, token: &str, name: &str, roles: Value) -> Value {
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/users",
            json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":roles}),
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

#[tokio::test]
async fn roles_validate_selected_references_and_return_readable_scope_labels() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let team = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let id = team["id"].as_str().unwrap();
    let scoped = role(
        &app,
        token,
        "scoped",
        json!(["users.read"]),
        "roles",
        json!([id]),
    )
    .await;
    assert_eq!(scoped["dataScopeRoleIds"], json!([id]));
    assert_eq!(scoped["dataScopeRoles"][0]["name"], "team-a");
    for (scope, ids) in [
        ("roles", json!([])),
        ("roles", json!([id, id])),
        ("roles", json!(["not-id"])),
        (
            "roles",
            json!([mongodb::bson::oid::ObjectId::new().to_hex()]),
        ),
        ("self", json!([id])),
        ("all", json!([id])),
    ] {
        let (status,body)=app.send("POST","/api/v1/roles",json!({"name":"invalid","slug":"invalid","permissions":[],"dataScope":scope,"dataScopeRoleIds":ids}),Some(token)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    let path = format!("/api/v1/roles/{}", scoped["id"].as_str().unwrap());
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"dataScope":"self"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, body) = app
        .send(
            "PATCH",
            &path,
            json!({"version":1,"dataScope":"self","dataScopeRoleIds":[]}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["dataScopeRoleIds"], json!([]));
    app.cleanup().await;
}

#[tokio::test]
async fn selected_role_scopes_aggregate_and_filter_users_before_pagination() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let a = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let b = role(&app, token, "team-b", json!([]), "self", json!([])).await;
    let c = role(&app, token, "team-c", json!([]), "self", json!([])).await;
    let scope_a = role(
        &app,
        token,
        "scope-a",
        json!(["users.read"]),
        "roles",
        json!([a["id"]]),
    )
    .await;
    let scope_b = role(&app, token, "scope-b", json!([]), "roles", json!([b["id"]])).await;
    let a_user = user(&app, token, "member-a", json!([a["id"]])).await;
    user(&app, token, "member-b", json!([b["id"]])).await;
    let outside = user(&app, token, "member-c", json!([c["id"]])).await;
    user(
        &app,
        token,
        "operator",
        json!([scope_a["id"], scope_b["id"]]),
    )
    .await;
    let session = login(&app, "operator").await;
    let access = session["accessToken"].as_str().unwrap();
    assert_eq!(session["user"]["dataScope"], "roles");
    assert_eq!(
        session["user"]["dataScopeRoleIds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (_, result) = app
        .send(
            "GET",
            "/api/v1/users?pageSize=1&sort=username",
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(result["meta"]["total"], 3);
    assert_eq!(result["data"].as_array().unwrap().len(), 1);
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/users/{}", a_user["id"].as_str().unwrap()),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/users/{}", outside["id"].as_str().unwrap()),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let path = format!("/api/v1/roles/{}", scope_a["id"].as_str().unwrap());
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"dataScope":"self","dataScopeRoleIds":[]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, me) = app
        .send("GET", "/api/v1/auth/me", Value::Null, Some(access))
        .await;
    assert_eq!(me["data"]["dataScopeRoleIds"], json!([b["id"]]));
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/users/{}", a_user["id"].as_str().unwrap()),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn operators_cannot_grant_or_assign_a_wider_role_scope() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let a = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let b = role(&app, token, "team-b", json!([]), "self", json!([])).await;
    let codes = json!([
        "users.read",
        "users.write",
        "roles.read",
        "roles.write",
        "roles.assign"
    ]);
    let scoped = role(
        &app,
        token,
        "operator-scope",
        codes.clone(),
        "roles",
        json!([a["id"]]),
    )
    .await;
    let wide = role(
        &app,
        token,
        "wide-scope",
        codes.clone(),
        "roles",
        json!([a["id"], b["id"]]),
    )
    .await;
    let target = user(&app, token, "member-a", json!([a["id"]])).await;
    let outside = user(&app, token, "member-b", json!([b["id"]])).await;
    user(&app, token, "operator", json!([scoped["id"]])).await;
    let session = login(&app, "operator").await;
    let access = session["accessToken"].as_str().unwrap();
    role(
        &app,
        access,
        "delegated",
        json!(["users.read"]),
        "roles",
        json!([a["id"]]),
    )
    .await;
    for (scope, ids) in [("roles", json!([b["id"]])), ("all", json!([]))] {
        assert_eq!(app.send("POST","/api/v1/roles",json!({"name":"escape","slug":"escape","permissions":[],"dataScope":scope,"dataScopeRoleIds":ids}),Some(access)).await.0,StatusCode::FORBIDDEN);
    }
    let target_path = format!("/api/v1/users/{}", target["id"].as_str().unwrap());
    assert_eq!(
        app.send(
            "PATCH",
            &target_path,
            json!({"version":1,"roleIds":[a["id"],wide["id"]]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "PATCH",
            &target_path,
            json!({"version":1,"roleIds":[b["id"]]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "PATCH",
            &target_path,
            json!({"version":1,"roleIds":[a["id"],b["id"]]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{}", outside["id"].as_str().unwrap()),
            json!({"version":1,"displayName":"escape"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    user(&app, access, "created-in-scope", json!([a["id"]])).await;
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"created-outside","displayName":"escape","password":PASSWORD,"roleIds":[b["id"]]}),Some(access)).await.0,StatusCode::FORBIDDEN);
    app.cleanup().await;
}

#[tokio::test]
async fn referenced_scope_roles_cannot_be_deleted_until_the_reference_is_removed() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let team = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let scoped = role(
        &app,
        token,
        "scope-a",
        json!([]),
        "roles",
        json!([team["id"]]),
    )
    .await;
    let path = format!("/api/v1/roles/{}?version=1", team["id"].as_str().unwrap());
    assert_eq!(
        app.send("DELETE", &path, Value::Null, Some(token)).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/roles/{}?version=1", scoped["id"].as_str().unwrap()),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("DELETE", &path, Value::Null, Some(token)).await.0,
        StatusCode::NO_CONTENT
    );
    app.cleanup().await;
}

#[tokio::test]
async fn new_project_rbac_v1_migrates_to_v2_without_runtime_shape_fallbacks() {
    let app = TestApp::new().await;
    app.database
        .collection::<Document>("roles")
        .update_many(doc! {}, doc! {"$unset":{"dataScopeRoleIds":""}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"rbac"}, doc! {"$set":{"version":1}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let roles = app.database.collection::<Document>("roles");
    assert_eq!(
        roles
            .count_documents(doc! {"dataScopeRoleIds":{"$exists":false}})
            .await
            .unwrap(),
        0
    );
    let auth = app.login().await;
    assert_eq!(auth["user"]["dataScopeRoleIds"], json!([]));
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn role_picker_search_finds_roles_beyond_the_first_page_using_literal_slug_prefixes() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let now = mongodb::bson::DateTime::now();
    let records=(0..110).map(|index|doc!{"name":"Filler","slug":format!("filler-{index:03}"),"permissions":[],"dataScope":"self","dataScopeRoleIds":[],"protected":false,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("roles")
        .insert_many(records)
        .await
        .unwrap();
    role(&app, token, "target-final", json!([]), "self", json!([])).await;
    let (status, result) = app
        .send(
            "GET",
            "/api/v1/roles?search=target-&sort=slug&pageSize=100",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["meta"]["total"], 1);
    assert_eq!(result["data"][0]["slug"], "target-final");
    let (_, result) = app
        .send(
            "GET",
            "/api/v1/roles?search=target.",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(result["meta"]["total"], 0);
    app.cleanup().await;
}

#[tokio::test]
async fn self_scope_cannot_grant_or_assign_selected_roles_and_all_scope_wins_aggregation() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let codes = json!(["roles.write", "roles.assign", "users.write"]);
    let team = role(&app, token, "team", json!([]), "self", json!([])).await;
    let limited = role(&app, token, "limited", codes.clone(), "self", json!([])).await;
    let selected = role(
        &app,
        token,
        "selected",
        codes.clone(),
        "roles",
        json!([team["id"]]),
    )
    .await;
    let own = user(&app, token, "self-operator", json!([limited["id"]])).await;
    let session = login(&app, "self-operator").await;
    let access = session["accessToken"].as_str().unwrap();
    assert_eq!(app.send("POST","/api/v1/roles",json!({"name":"escape","slug":"escape","permissions":[],"dataScope":"roles","dataScopeRoleIds":[team["id"]]}),Some(access)).await.0,StatusCode::FORBIDDEN);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{}", own["id"].as_str().unwrap()),
            json!({"version":1,"roleIds":[limited["id"],selected["id"]]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let all = role(&app, token, "all-records", json!([]), "all", json!([])).await;
    user(&app, token, "union-all", json!([selected["id"], all["id"]])).await;
    let session = login(&app, "union-all").await;
    assert_eq!(session["user"]["dataScope"], "all");
    assert_eq!(session["user"]["dataScopeRoleIds"], json!([]));
    app.cleanup().await;
}

#[tokio::test]
async fn assignment_choices_are_scoped_and_do_not_require_global_role_read_access() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let a = role(&app, token, "team-a", json!([]), "self", json!([])).await;
    let b = role(&app, token, "team-b", json!([]), "self", json!([])).await;
    let scoped = role(
        &app,
        token,
        "limited-assigner",
        json!(["users.read", "roles.assign"]),
        "roles",
        json!([a["id"]]),
    )
    .await;
    role(
        &app,
        token,
        "wide-grant",
        json!(["users.read"]),
        "roles",
        json!([a["id"], b["id"]]),
    )
    .await;
    user(&app, token, "assigner", json!([scoped["id"]])).await;
    let session = login(&app, "assigner").await;
    let access = session["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/roles", Value::Null, Some(access))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, result) = app
        .send(
            "GET",
            "/api/v1/roles/assignable?sort=slug",
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let data = result["data"].as_array().unwrap();
    assert_eq!(result["meta"]["total"], 2);
    assert!(data.iter().any(|value| value["id"] == a["id"]));
    assert!(data.iter().any(|value| value["id"] == scoped["id"]));
    assert!(
        data.iter()
            .all(|value| value.get("permissions").is_none() && value.get("dataScope").is_none())
    );
    let (_, filtered) = app
        .send(
            "GET",
            "/api/v1/roles/assignable?search=team-&pageSize=1",
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(filtered["meta"]["total"], 1);
    assert_eq!(filtered["data"][0]["id"], a["id"]);
    let (_, auth) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"assigner","password":PASSWORD}),
            None,
        )
        .await;
    assert!(auth["data"]["accessToken"].is_string());
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_scope_reference_creation_and_role_deletion_cannot_leave_a_dangling_reference() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let team = role(&app, token, "race-team", json!([]), "self", json!([])).await;
    let path = format!("/api/v1/roles/{}?version=1", team["id"].as_str().unwrap());
    let (delete,create)=tokio::join!(app.send("DELETE",&path,Value::Null,Some(token)),app.send("POST","/api/v1/roles",json!({"name":"race","slug":"race-scope","permissions":[],"dataScope":"roles","dataScopeRoleIds":[team["id"]]}),Some(token)));
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
async fn role_list_and_scope_reference_queries_use_bounded_indexes_on_a_populated_collection() {
    let app = TestApp::new().await;
    let target = mongodb::bson::oid::ObjectId::new();
    let now = mongodb::bson::DateTime::now();
    app.database.collection::<Document>("roles").insert_one(doc!{"_id":target,"name":"Reference target","slug":"reference-target","permissions":[],"dataScope":"self","dataScopeRoleIds":[],"protected":false,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).await.unwrap();
    let records = (0..2000)
        .map(|index| {
            doc! {
                "name":"Query role","slug":format!("query-{index:04}"),"permissions":[],
                "dataScope":if index<100 {"roles"} else {"self"},
                "dataScopeRoleIds":if index<100 {vec![target]} else {vec![]},
                "protected":false,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null
            }
        })
        .collect::<Vec<_>>();
    app.database
        .collection::<Document>("roles")
        .insert_many(records)
        .await
        .unwrap();
    for (filter, sort) in [
        (doc! {"deletedAt":null}, doc! {"createdAt":-1,"_id":-1}),
        (doc! {"deletedAt":null,"dataScopeRoleIds":target}, doc! {}),
    ] {
        let mut query = doc! {"find":"roles","filter":filter,"limit":20};
        if !sort.is_empty() {
            query.insert("sort", sort);
        }
        let explain = app
            .database
            .run_command(doc! {"explain":query,"verbosity":"executionStats"})
            .await
            .unwrap();
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
            "roles: returned={},docs={},keys={} of 2000",
            number("nReturned"),
            number("totalDocsExamined"),
            number("totalKeysExamined")
        );
    }
    app.cleanup().await;
}
