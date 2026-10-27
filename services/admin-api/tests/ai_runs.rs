mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
async fn setup() -> (TestApp, String, Value) {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap().to_owned();
    let (status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"AI runs","code":"ai.runs","enabled":true,"configuration":{"kind":"http","baseUrl":"http://127.0.0.1:9/v1/responses"}}),Some(&token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status,provider)=app.send("POST","/api/v1/ai/providers",json!({"name":"Run provider","enabled":true,"profileId":profile["data"]["id"],"profileVersion":1,"models":[{"id":"fixture-model","label":"Fixture","maxOutputTokens":512}]}),Some(&token)).await;
    assert_eq!(status, StatusCode::CREATED, "{provider}");
    (app, token, provider["data"].clone())
}
fn input(provider: &Value) -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"providerId":provider["id"],"providerVersion":provider["version"],"model":"fixture-model","input":"PRIVATE-REQUIREMENTS","instructions":"PRIVATE-INSTRUCTIONS","maxOutputTokens":128,"stream":true})
}
async fn run(app: &TestApp, token: &str, body: Value) -> Value {
    let (status, value) = app.send("POST", "/api/v1/ai/runs", body, Some(token)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{value}");
    value["data"].clone()
}
async fn account(app: &TestApp, owner: &str, name: &str, scope: &str) -> String {
    scoped_account(app, owner, name, scope, json!([])).await
}
async fn scoped_account(
    app: &TestApp,
    owner: &str,
    name: &str,
    scope: &str,
    selected: Value,
) -> String {
    let (status,role)=app.send("POST","/api/v1/roles",json!({"name":name,"slug":name,"permissions":["ai.read","ai.invoke"],"dataScope":scope,"dataScopeRoleIds":selected}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
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
async fn accepted_run_has_one_encrypted_intent_exact_uuid_recovery_and_durable_queued_cancel() {
    let (mut app, token, provider) = setup().await;
    let body = input(&provider);
    let row = run(&app, &token, body.clone()).await;
    assert_eq!(row["state"], "pending");
    assert!(!row.to_string().contains("PRIVATE"));
    assert_eq!(run(&app, &token, body.clone()).await["id"], row["id"]);
    let mut changed = body.clone();
    changed["input"] = json!("different");
    assert_eq!(
        app.send("POST", "/api/v1/ai/runs", changed, Some(&token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let path = format!("/api/v1/ai/runs/{}", row["id"].as_str().unwrap());
    let recovered = app
        .send(
            "GET",
            &format!(
                "/api/v1/ai/runs/by-request/{}",
                body["requestId"].as_str().unwrap()
            ),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(recovered.0, StatusCode::OK);
    assert_eq!(recovered.1["data"]["id"], row["id"]);
    let content = app
        .send(
            "GET",
            &(path.clone() + "/content"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(content.0, StatusCode::OK);
    assert_eq!(content.1["data"]["input"], "PRIVATE-REQUIREMENTS");
    let original = app.router.clone();
    let no_keys = admin_api::config::Config::from_lookup(|name| {
        if name == "TRI_JWT_SIGNING_KEY" {
            Some(support::KEY.into())
        } else {
            None
        }
    })
    .unwrap();
    app.router = admin_api::build_router(admin_api::state::AppState::new(
        no_keys,
        Some(app.database.clone()),
    ));
    let denied = app
        .send(
            "GET",
            &(path.clone() + "/content"),
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(denied.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!denied.1.to_string().contains("PRIVATE-"));
    app.router = original;
    for collection in ["ai_runs", "ai_inputs", "ai_states"] {
        let rows = app
            .database
            .collection::<Document>(collection)
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap();
        assert!(!format!("{rows:?}").contains("PRIVATE-"));
        assert_eq!(
            app.database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
    }
    let cancel = json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1});
    let cancelled = app
        .send(
            "POST",
            &(path.clone() + "/cancel"),
            cancel.clone(),
            Some(&token),
        )
        .await;
    assert_eq!(cancelled.0, StatusCode::ACCEPTED);
    assert_eq!(cancelled.1["data"]["state"], "cancelled");
    assert_eq!(cancelled.1["data"]["mayHaveBeenSubmitted"], false);
    assert_eq!(
        app.send("POST", &(path + "/cancel"), cancel, Some(&token))
            .await
            .1["data"]["id"],
        row["id"]
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"ai.runs.cancel"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn metadata_is_scoped_before_count_and_only_original_current_owner_reads_private_content() {
    let (app, token, provider) = setup().await;
    let row = run(&app, &token, input(&provider)).await;
    let path = format!("/api/v1/ai/runs/{}", row["id"].as_str().unwrap());
    let own = account(&app, &token, "ai-own-runner", "self").await;
    let all = account(&app, &token, "ai-all-reader", "all").await;
    let page = app
        .send("GET", "/api/v1/ai/runs?pageSize=1", Value::Null, Some(&own))
        .await;
    assert_eq!(page.0, StatusCode::OK);
    assert_eq!(page.1["meta"]["total"], 0);
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(&own)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send("GET", "/api/v1/ai/runs?pageSize=1", Value::Null, Some(&all))
            .await
            .1["meta"]["total"],
        1
    );
    assert_eq!(
        app.send("GET", &(path.clone() + "/content"), Value::Null, Some(&all))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let own_row = run(&app, &own, input(&provider)).await;
    let selected_role = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"ai-own-runner"})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("_id")
        .unwrap();
    let selected = scoped_account(
        &app,
        &token,
        "ai-selected-reader",
        "roles",
        json!([selected_role.to_hex()]),
    )
    .await;
    let selected_page = app
        .send(
            "GET",
            "/api/v1/ai/runs?pageSize=1",
            Value::Null,
            Some(&selected),
        )
        .await;
    assert_eq!(selected_page.1["meta"]["total"], 1);
    assert_eq!(selected_page.1["data"][0]["id"], own_row["id"]);
    let private_path = format!(
        "/api/v1/ai/runs/{}/content",
        own_row["id"].as_str().unwrap()
    );
    assert_eq!(
        app.send("GET", &private_path, Value::Null, Some(&own))
            .await
            .0,
        StatusCode::OK
    );
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"slug":"ai-own-runner"},
            doc! {"$set":{"permissions":["ai.read"]}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", &private_path, Value::Null, Some(&own))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    app.database
        .collection::<Document>("users")
        .update_one(
            doc! {"username":"ai-own-runner"},
            doc! {"$set":{"roleIds":[]}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/ai/runs?pageSize=1",
            Value::Null,
            Some(&selected)
        )
        .await
        .1["meta"]["total"],
        0
    );
    app.cleanup().await;
}
#[tokio::test]
async fn failed_authorization_approval_or_audit_does_not_admit_or_leave_private_blobs() {
    let (mut app, token, provider) = setup().await;
    let mut invalid = input(&provider);
    invalid["model"] = json!("not-approved");
    assert_eq!(
        app.send("POST", "/api/v1/ai/runs", invalid, Some(&token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut excessive = input(&provider);
    excessive["maxOutputTokens"] = json!(513);
    assert_eq!(
        app.send("POST", "/api/v1/ai/runs", excessive, Some(&token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let original = app.router.clone();
    let no_keys = admin_api::config::Config::from_lookup(|name| {
        if name == "TRI_JWT_SIGNING_KEY" {
            Some(support::KEY.into())
        } else {
            None
        }
    })
    .unwrap();
    app.router = admin_api::build_router(admin_api::state::AppState::new(
        no_keys,
        Some(app.database.clone()),
    ));
    assert_eq!(
        app.send("POST", "/api/v1/ai/runs", input(&provider), Some(&token))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.router = original;
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"ai.runs.create"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send("POST", "/api/v1/ai/runs", input(&provider), Some(&token))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for collection in ["ai_runs", "ai_inputs", "ai_states"] {
        assert_eq!(
            app.database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    app.cleanup().await;
}
#[tokio::test]
async fn concurrent_admission_enforces_the_owner_limit_without_orphan_private_payloads() {
    let (app, token, provider) = setup().await;
    let bodies = (0..24).map(|_| input(&provider)).collect::<Vec<_>>();
    let results = futures_util::future::join_all(
        bodies
            .into_iter()
            .map(|body| app.send("POST", "/api/v1/ai/runs", body, Some(&token))),
    )
    .await;
    assert_eq!(
        results
            .iter()
            .filter(|value| value.0 == StatusCode::ACCEPTED)
            .count(),
        20
    );
    assert_eq!(
        results
            .iter()
            .filter(|value| value.0 == StatusCode::TOO_MANY_REQUESTS)
            .count(),
        4
    );
    for collection in ["ai_runs", "ai_inputs", "ai_states"] {
        assert_eq!(
            app.database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            20
        );
    }
    app.cleanup().await;
}
#[tokio::test]
async fn expired_private_payload_keeps_the_original_request_receipt_and_never_recreates_it() {
    let (app, token, provider) = setup().await;
    let body = input(&provider);
    let row = run(&app, &token, body.clone()).await;
    let id = row["id"].as_str().unwrap();
    app.database
        .collection::<Document>("ai_runs")
        .update_one(
            doc! {"_id":mongodb::bson::oid::ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/ai/runs/{id}/content"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(run(&app, &token, body).await["id"], id);
    assert_eq!(
        app.database
            .collection::<Document>("ai_runs")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
