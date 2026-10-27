mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
fn definition() -> Value {
    json!({"mode":"project","steps":[{"id":"analysis","title":"Analysis","prompt":"PRIVATE-PLAN-PROMPT","dependsOn":[],"expected":"analysis"},{"id":"deliver","title":"Deliver","prompt":"Deliver concrete implementation","dependsOn":["analysis"],"expected":"text"}]})
}
fn input() -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Private plan","plan":definition()})
}
async fn create(app: &TestApp, token: &str, body: Value) -> Value {
    let (status, value) = app
        .send("POST", "/api/v1/workflows/plans", body, Some(token))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
async fn account(app: &TestApp, owner: &str, name: &str, scope: &str) -> String {
    let (status,role)=app.send("POST","/api/v1/roles",json!({"name":name,"slug":name,"permissions":["workflows.read","workflows.write"],"dataScope":scope}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await.0,StatusCode::CREATED);
    app.send(
        "POST",
        "/api/v1/auth/login",
        json!({"username":name,"password":PASSWORD}),
        None,
    )
    .await
    .1["data"]["accessToken"]
        .as_str()
        .unwrap()
        .into()
}
#[tokio::test]
async fn templates_and_encrypted_plan_revisions_are_real_versioned_resources() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (status, templates) = app
        .send(
            "GET",
            "/api/v1/workflows/templates",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(templates["data"].as_array().unwrap().len(), 3);
    let body = input();
    let row = create(&app, token, body.clone()).await;
    let id = row["id"].as_str().unwrap();
    assert_eq!(row["stepCount"], 2);
    assert_eq!(row["version"], 1);
    assert!(!row.to_string().contains("PRIVATE-PLAN-PROMPT"));
    let detail = app
        .send(
            "GET",
            &format!("/api/v1/workflows/plans/{id}/definition"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(detail.0, StatusCode::OK);
    assert_eq!(
        detail.1["data"]["plan"]["steps"][0]["prompt"],
        "PRIVATE-PLAN-PROMPT"
    );
    let mut next = definition();
    next["steps"][0]["prompt"] = json!("PRIVATE-PLAN-REVISION-TWO");
    let patch = json!({"version":1,"name":"Updated plan","plan":next});
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/workflows/plans/{id}"),
            patch.clone(),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/workflows/plans/{id}"),
            patch,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let original = app
        .send(
            "GET",
            &format!("/api/v1/workflows/plans/{id}/revisions/1"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(original.0, StatusCode::OK);
    assert_eq!(
        original.1["data"]["plan"]["steps"][0]["prompt"],
        "PRIVATE-PLAN-PROMPT"
    );
    assert_eq!(create(&app, token, body).await["id"], id);
    let mut cursor = app
        .database
        .collection::<Document>("workflow_plan_revisions")
        .find(doc! {})
        .await
        .unwrap();
    let mut count = 0;
    while cursor.advance().await.unwrap() {
        let row = cursor.deserialize_current().unwrap();
        assert!(!format!("{row:?}").contains("PRIVATE-PLAN"));
        count += 1;
    }
    assert_eq!(count, 2);
    app.cleanup().await;
}
#[tokio::test]
async fn plan_metadata_is_scoped_and_raw_prompts_remain_private_to_the_current_owner() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let row = create(&app, owner, input()).await;
    let id = row["id"].as_str().unwrap();
    let own = account(&app, owner, "workflow-own", "self").await;
    let all = account(&app, owner, "workflow-all", "all").await;
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/workflows/plans?pageSize=1",
            Value::Null,
            Some(&own)
        )
        .await
        .1["meta"]["total"],
        0
    );
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/workflows/plans?pageSize=1",
            Value::Null,
            Some(&all)
        )
        .await
        .1["meta"]["total"],
        1
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/workflows/plans/{id}/definition"),
            Value::Null,
            Some(&all)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/workflows/plans/{id}"),
            json!({"version":1,"name":"stolen","plan":definition()}),
            Some(&all)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.cleanup().await;
}
#[tokio::test]
async fn invalid_dependencies_request_conflicts_and_audit_rollback_leave_no_private_orphans() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let body = input();
    let row = create(&app, token, body.clone()).await;
    let mut changed = body;
    changed["name"] = json!("different");
    assert_eq!(
        app.send("POST", "/api/v1/workflows/plans", changed, Some(token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let mut cyclic = input();
    cyclic["plan"]["steps"][0]["dependsOn"] = json!(["deliver"]);
    assert_eq!(
        app.send("POST", "/api/v1/workflows/plans", cyclic, Some(token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"workflows.plans.create"}},"validationLevel":"strict"}).await.unwrap();
    let mut second = input();
    second["name"] = json!("failed audit");
    assert_eq!(
        app.send("POST", "/api/v1/workflows/plans", second, Some(token))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for collection in ["workflow_plans", "workflow_plan_revisions"] {
        assert_eq!(
            app.database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
    }
    assert!(!row["id"].is_null());
    app.cleanup().await;
}
#[tokio::test]
async fn deleted_plans_do_not_reveal_private_definitions_or_reuse_old_creation_uuids() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let body = input();
    let row = create(&app, token, body.clone()).await;
    let id = row["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/workflows/plans/{id}?version=1"),
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
            &format!("/api/v1/workflows/plans/{id}/definition"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let replay = create(&app, token, body).await;
    assert_eq!(replay["id"], id);
    assert!(replay["deletedAt"].is_string());
    app.cleanup().await;
}
