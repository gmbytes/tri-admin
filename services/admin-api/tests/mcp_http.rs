mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

async fn profile(app: &TestApp, token: &str) -> String {
    let (status,body)=app.send("POST","/api/v1/settings/profiles",json!({
        "name":"MCP HTTP fixture","code":"mcp.http.fixture","enabled":true,
        "configuration":{"kind":"http","baseUrl":"http://127.0.0.1:9/mcp"},
        "secret":{"kind":"http","headerName":"authorization","value":"Bearer synthetic-mcp-http-credential"}
    }),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(!body.to_string().contains("synthetic-mcp-http-credential"));
    body["data"]["id"].as_str().unwrap().into()
}
fn connection(profile: &str, request: &str) -> Value {
    json!({"requestId":request,"name":"HTTP MCP","transport":{"kind":"http","profileId":profile,"profileVersion":1,"allowedTools":["increment","inspect"]}})
}

#[tokio::test]
async fn http_connections_bind_encrypted_profiles_and_prevent_dangling_references() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token).await;
    let request = uuid::Uuid::new_v4().to_string();
    let input = connection(&profile, &request);
    let (status, created) = app
        .send(
            "POST",
            "/api/v1/mcp/connections",
            input.clone(),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["data"]["transport"]["kind"], "http");
    assert_eq!(created["data"]["transport"]["profileId"], profile);
    assert_eq!(created["data"]["capabilityCurrent"], true);
    assert_eq!(created["data"]["runtime"], Value::Null);
    assert_eq!(created["data"]["enabled"], false);
    assert!(
        !created
            .to_string()
            .contains("synthetic-mcp-http-credential")
    );
    assert!(!created.to_string().contains("baseUrl"));
    let id = created["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/settings/profiles/{profile}?version=1"),
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
            &format!("/api/v1/settings/profiles/{profile}"),
            json!({"version":1,"enabled":false}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{profile}"),
            json!({"version":1,"name":"profile metadata changed"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, current) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["data"]["capabilityCurrent"], false);
    let (status, replay) = app
        .send("POST", "/api/v1/mcp/connections", input, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["id"], id);
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/mcp/connections/{id}?version=1"),
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
            &format!("/api/v1/settings/profiles/{profile}?version=2"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let record = app
        .database
        .collection::<Document>("integration_profiles")
        .find_one(doc! {"_id":ObjectId::parse_str(&profile).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert!(record.contains_key("secretBox"));
    assert!(!format!("{record:?}").contains("synthetic-mcp-http-credential"));
    app.cleanup().await;
}

#[tokio::test]
async fn http_binding_requires_configuration_authority_and_never_accepts_inline_secrets_or_urls() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, admin).await;
    let (status,role)=app.send("POST","/api/v1/roles",json!({"name":"MCP writer","slug":"mcp-writer-http","permissions":["mcp.read","mcp.write","mcp.invoke"],"dataScope":"all"}),Some(admin)).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let (status,user)=app.send("POST","/api/v1/users",json!({"username":"mcp-http-writer","displayName":"HTTP writer","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(admin)).await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    let (_, auth) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"mcp-http-writer","password":PASSWORD}),
            None,
        )
        .await;
    let token = auth["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/mcp/connections",
            connection(&profile, &uuid::Uuid::new_v4().to_string()),
            Some(token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for extra in [
        json!({"url":"https://unapproved.invalid/mcp"}),
        json!({"credentials":{"authorization":"inline-secret"}}),
    ] {
        let mut input = connection(&profile, &uuid::Uuid::new_v4().to_string());
        input["transport"]
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            app.send("POST", "/api/v1/mcp/connections", input, Some(admin))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        app.database
            .collection::<Document>("mcp_connections")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn profile_deletion_racing_a_binding_cannot_commit_a_dangling_reference() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token).await;
    let input = connection(&profile, &uuid::Uuid::new_v4().to_string());
    let path = format!("/api/v1/settings/profiles/{profile}?version=1");
    let (bound, deleted) = tokio::join!(
        app.send("POST", "/api/v1/mcp/connections", input, Some(token)),
        app.send("DELETE", &path, Value::Null, Some(token))
    );
    assert!(
        matches!(
            (bound.0, deleted.0),
            (StatusCode::CREATED, StatusCode::CONFLICT)
                | (StatusCode::UNPROCESSABLE_ENTITY, StatusCode::NO_CONTENT)
        ),
        "{} {}",
        bound.1,
        deleted.1
    );
    let count = app
        .database
        .collection::<Document>("mcp_connections")
        .count_documents(doc! {"deletedAt":null})
        .await
        .unwrap();
    assert_eq!(count, if bound.0 == StatusCode::CREATED { 1 } else { 0 });
    app.cleanup().await;
}

#[tokio::test]
async fn profile_credentials_cannot_override_mcp_protocol_headers_or_duplicate_tool_grants() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = profile(&app, token).await;
    let mut input = connection(&profile, &uuid::Uuid::new_v4().to_string());
    input["transport"]["allowedTools"] = json!(["increment", "increment"]);
    assert_eq!(
        app.send("POST", "/api/v1/mcp/connections", input, Some(token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status,body)=app.send("PATCH",&format!("/api/v1/settings/profiles/{profile}"),json!({"version":1,"secret":{"kind":"http","headerName":"Mcp-Method","value":"tools/call"}}),Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut input = connection(&profile, &uuid::Uuid::new_v4().to_string());
    input["transport"]["profileVersion"] = json!(2);
    assert_eq!(
        app.send("POST", "/api/v1/mcp/connections", input, Some(token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.database
            .collection::<Document>("mcp_connections")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}
