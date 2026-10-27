mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

async fn profile(app: &TestApp, token: &str, code: &str) -> Value {
    let (status,value)=app.send("POST","/api/v1/settings/profiles",json!({"name":code,"code":code,"enabled":true,"configuration":{"kind":"http","baseUrl":"http://127.0.0.1:9/v1/responses"},"secret":{"kind":"http","headerName":"authorization","value":"Bearer PRIVATE-AI-FIXTURE"}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
fn input(profile: &Value) -> Value {
    json!({"name":"Approved fixture","profileId":profile["id"],"profileVersion":1,"enabled":true,"models":[{"id":"fixture-text","label":"Fixture text","maxOutputTokens":1024}]})
}
async fn provider(app: &TestApp, token: &str, profile: &Value) -> Value {
    let (status, value) = app
        .send("POST", "/api/v1/ai/providers", input(profile), Some(token))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
async fn account(
    app: &TestApp,
    owner: &str,
    name: &str,
    permissions: Value,
    scope: &str,
) -> String {
    let (status, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":name,"slug":name,"permissions":permissions,"dataScope":scope}),
            Some(owner),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let (status,value)=app.send("POST","/api/v1/users",json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    let (status, value) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    value["data"]["accessToken"].as_str().unwrap().into()
}
#[tokio::test]
async fn approved_models_are_versioned_without_network_calls_or_credential_disclosure() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, "ai.fixture").await;
    let row = provider(&app, token, &profile).await;
    assert_eq!(row["version"], 1);
    assert_eq!(row["bindingCurrent"], true);
    assert!(!row.to_string().contains("PRIVATE-AI-FIXTURE"));
    assert!(!row.to_string().contains("127.0.0.1"));
    let (status, models) = app
        .send("GET", "/api/v1/ai/models", Value::Null, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(models["data"].as_array().unwrap().len(), 1);
    assert_eq!(models["data"][0]["id"], "fixture-text");
    assert_eq!(models["data"][0]["providerId"], row["id"]);
    let (status, list) = app
        .send(
            "GET",
            "/api/v1/ai/providers?search=Approved&pageSize=1",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["meta"]["total"], 1);
    let id = row["id"].as_str().unwrap();
    let mut patch = input(&profile);
    patch["version"] = json!(1);
    patch["enabled"] = json!(false);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/ai/providers/{id}"),
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
            &format!("/api/v1/ai/providers/{id}"),
            patch,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(
        app.send("GET", "/api/v1/ai/models", Value::Null, Some(token))
            .await
            .1["data"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let path = format!(
        "/api/v1/settings/profiles/{}?version=1",
        profile["id"].as_str().unwrap()
    );
    assert_eq!(
        app.send("DELETE", &path, Value::Null, Some(token)).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/ai/providers/{id}?version=2"),
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
async fn provider_maintenance_requires_current_ai_and_settings_permissions_and_all_scope() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, "ai.scope").await;
    provider(&app, token, &profile).await;
    for (name, permissions, scope) in [
        ("ai-reader", json!(["ai.read"]), "all"),
        (
            "ai-own",
            json!(["ai.read", "ai.write", "settings.write"]),
            "self",
        ),
        ("ai-no-settings", json!(["ai.read", "ai.write"]), "all"),
    ] {
        let actor = account(&app, token, name, permissions, scope).await;
        assert_eq!(
            app.send("GET", "/api/v1/ai/models", Value::Null, Some(&actor))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/ai/providers",
                input(&profile),
                Some(&actor)
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let actor = account(
        &app,
        token,
        "ai-manager",
        json!(["ai.read", "ai.write", "settings.write"]),
        "all",
    )
    .await;
    let mut body = input(&profile);
    body["name"] = json!("Manager configured");
    assert_eq!(
        app.send("POST", "/api/v1/ai/providers", body, Some(&actor))
            .await
            .0,
        StatusCode::CREATED
    );
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"slug":"ai-manager"},
            doc! {"$set":{"permissions":["ai.read"]}},
        )
        .await
        .unwrap();
    let mut body = input(&profile);
    body["name"] = json!("Revoked");
    assert_eq!(
        app.send("POST", "/api/v1/ai/providers", body, Some(&actor))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}
#[tokio::test]
async fn invalid_models_stale_bindings_and_audit_failure_cannot_create_provider_rows() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, "ai.invalid").await;
    assert_eq!(
        app.send("GET", "/api/v1/ai/models", Value::Null, Some(token))
            .await
            .0,
        StatusCode::OK
    );
    for models in [
        json!([]),
        json!([{"id":"fixture-text","label":"x","maxOutputTokens":0}]),
        json!([{"id":"../invalid model","label":"x","maxOutputTokens":8}]),
        json!([{"id":"same","label":"x","maxOutputTokens":8},{"id":"same","label":"y","maxOutputTokens":8}]),
    ] {
        let mut body = input(&profile);
        body["models"] = models;
        assert_eq!(
            app.send("POST", "/api/v1/ai/providers", body, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut stale = input(&profile);
    stale["profileVersion"] = json!(2);
    assert_eq!(
        app.send("POST", "/api/v1/ai/providers", stale, Some(token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"ai.providers.create"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send("POST", "/api/v1/ai/providers", input(&profile), Some(token))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("ai_providers")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}
#[tokio::test]
async fn changing_profile_version_hides_stale_models_until_explicit_rebinding() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token, "ai.binding").await;
    let row = provider(&app, token, &profile).await;
    let profile_path = format!(
        "/api/v1/settings/profiles/{}",
        profile["id"].as_str().unwrap()
    );
    assert_eq!(
        app.send(
            "PATCH",
            &profile_path,
            json!({"version":1,"description":"changed"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        app.send("GET", "/api/v1/ai/models", Value::Null, Some(token))
            .await
            .1["data"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let path = format!("/api/v1/ai/providers/{}", row["id"].as_str().unwrap());
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(token)).await.1["data"]["bindingCurrent"],
        false
    );
    let mut patch = input(&profile);
    patch["version"] = json!(1);
    patch["profileVersion"] = json!(2);
    assert_eq!(
        app.send("PATCH", &path, patch, Some(token)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("GET", "/api/v1/ai/models", Value::Null, Some(token))
            .await
            .1["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    app.cleanup().await;
}
