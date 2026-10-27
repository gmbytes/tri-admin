mod support;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
use tower::ServiceExt;
use tri_extension_package::package::{self, File};

fn archive(name: &str, tools: Value) -> String {
    let manifest = json!({"formatVersion":1,"name":name,"version":"1.0.0","description":"Fixture plugin","skills":[],"mcp":[],"tools":tools});
    let package = package::repack(
        name,
        vec![
            File::new(
                "tri-plugin.json".into(),
                serde_json::to_vec(&manifest).unwrap(),
                0o644,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    STANDARD.encode(package.archive)
}
fn tools() -> Value {
    json!([
        {"name":"search","description":"Search managed parameters","operation":"parameters.list","inputSchema":{"type":"object","properties":{"prefix":{"type":"string"}},"additionalProperties":false},"mappings":{"search":"prefix"}},
        {"name":"make","description":"Create managed parameter","operation":"parameters.create","inputSchema":{"type":"object","properties":{"label":{"type":"string"},"key":{"type":"string"},"kind":{"enum":["string","json"]},"value":{}},"required":["label","key","kind","value"],"additionalProperties":false},"mappings":{"name":"label","key":"key","valueType":"kind","value":"value"}}
    ])
}
async fn install(app: &TestApp, token: &str, name: &str, tools: Value) -> Value {
    let (status,body)=wide_send(app,"POST","/api/v1/extensions",json!({"requestId":uuid::Uuid::new_v4().to_string(),"source":{"kind":"archive","contentBase64":archive(name,tools)}}),token).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}
async fn wide_send(
    app: &TestApp,
    method: &str,
    path: &str,
    body: Value,
    token: &str,
) -> (StatusCode, Value) {
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn enable(app: &TestApp, token: &str, row: &Value) {
    let (status, body) = app
        .send(
            "PATCH",
            &format!("/api/v1/extensions/{}", row["id"].as_str().unwrap()),
            json!({"version":row["version"],"enabled":true}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
fn tool_name(row: &Value, name: &str) -> String {
    format!(
        "plugin.{}.{}.{name}",
        row["id"].as_str().unwrap(),
        row["revisionId"].as_str().unwrap()
    )
}
async fn rpc(app: &TestApp, token: &str, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"plugin-test","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/mcp/server")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", method)
        .header("authorization", format!("Bearer {token}"));
    if let Some(name) = params["name"].as_str() {
        request = request.header("mcp-name", name)
    }
    let response = app
        .router
        .clone()
        .oneshot(
            request
                .body(Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body.get("error").is_none(), "{body}");
    body["result"].clone()
}
async fn account(
    app: &TestApp,
    admin: &str,
    name: &str,
    permissions: Value,
    scope: &str,
) -> (String, String) {
    let (status, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":name,"slug":name,"permissions":permissions,"dataScope":scope}),
            Some(admin),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let role_id = role["data"]["id"].as_str().unwrap();
    let (status, user) = app
        .send(
            "POST",
            "/api/v1/users",
            json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role_id]}),
            Some(admin),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    let (status, signed) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    (
        signed["data"]["accessToken"].as_str().unwrap().into(),
        role_id.into(),
    )
}

#[tokio::test]
async fn enabled_aliases_run_actual_business_handlers_with_the_original_actor() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = install(&app, token, "actual-plugin", tools()).await;
    assert_eq!(row["enabled"], false);
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        rpc(&app, token, "tools/list", json!({})).await["tools"]
            .as_array()
            .unwrap()
            .len(),
        25
    );
    enable(&app, token, &row).await;
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let catalog = rpc(&app, token, "tools/list", json!({})).await;
    assert!(
        catalog["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == tool_name(&row, "make"))
    );
    let unsafe_number=rpc(&app,token,"tools/call",json!({"name":tool_name(&row,"make"),"arguments":{"label":"Plugin maximum","key":"plugin.maximum","kind":"json","value":18446744073709551615u64}})).await;
    assert_eq!(
        unsafe_number["isError"], true,
        "Underlying parameter policies must remain authoritative"
    );
    assert_eq!(
        unsafe_number["structuredContent"]["error"]["code"],
        "INVALID_INPUT"
    );
    let made=rpc(&app,token,"tools/call",json!({"name":tool_name(&row,"make"),"arguments":{"label":"Plugin maximum","key":"plugin.maximum","kind":"json","value":{"count":9007199254740991u64,"playerId":"18446744073709551615"}}})).await;
    assert_eq!(made["isError"], false, "{made}");
    assert_eq!(
        made["structuredContent"]["data"]["value"],
        json!({"count":9007199254740991u64,"playerId":"18446744073709551615"})
    );
    let found = rpc(
        &app,
        token,
        "tools/call",
        json!({"name":tool_name(&row,"search"),"arguments":{"prefix":"plugin."}}),
    )
    .await;
    assert_eq!(found["structuredContent"]["meta"]["total"], 1);
    assert_eq!(app.database.collection::<Document>("audit_events").count_documents(doc!{"action":"parameters.create","actorId":ObjectId::parse_str(auth["user"]["id"].as_str().unwrap()).unwrap()}).await.unwrap(),1);
    app.cleanup().await;
}

#[tokio::test]
async fn business_permissions_scope_and_live_revocation_control_plugin_calls() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap();
    let row = install(&app, admin, "scoped-plugin", tools()).await;
    enable(&app, admin, &row).await;
    let (reader, role) = account(
        &app,
        admin,
        "plugin-reader",
        json!(["skills.read", "mcp.read", "mcp.invoke", "parameters.read"]),
        "all",
    )
    .await;
    let list = rpc(&app, &reader, "tools/list", json!({})).await;
    assert!(
        list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == tool_name(&row, "search"))
    );
    assert!(
        !list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == tool_name(&row, "make"))
    );
    let denied=rpc(&app,&reader,"tools/call",json!({"name":tool_name(&row,"make"),"arguments":{"label":"Forbidden","key":"forbidden","kind":"string","value":"x"}})).await;
    assert_eq!(denied["isError"], true);
    assert_eq!(denied["structuredContent"]["error"]["code"], "FORBIDDEN");
    let (own, _) = account(
        &app,
        admin,
        "plugin-own",
        json!([
            "skills.read",
            "mcp.read",
            "mcp.invoke",
            "parameters.read",
            "parameters.write"
        ]),
        "self",
    )
    .await;
    assert!(
        !rpc(&app, &own, "tools/list", json!({})).await["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"].as_str().is_some_and(|s| s.starts_with("plugin.")))
    );
    assert_eq!(
        rpc(
            &app,
            &own,
            "tools/call",
            json!({"name":tool_name(&row,"search"),"arguments":{}})
        )
        .await["isError"],
        true
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{role}"),
            json!({"version":1,"permissions":["skills.read","mcp.read","mcp.invoke"]}),
            Some(admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        rpc(
            &app,
            &reader,
            "tools/call",
            json!({"name":tool_name(&row,"search"),"arguments":{}})
        )
        .await["isError"],
        true
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn revision_changes_and_disabling_never_redirect_an_old_tool_name() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = install(&app, token, "revision-plugin", tools()).await;
    enable(&app, token, &row).await;
    let(status,new)=app.send("POST",&format!("/api/v1/extensions/{}/revisions",row["id"].as_str().unwrap()),json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":2,"source":{"kind":"archive","contentBase64":archive("revision-plugin",tools())}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{new}");
    let current = new["data"].clone();
    assert_eq!(current["enabled"], false);
    let call = json!({"name":tool_name(&row,"make"),"arguments":{"label":"Old","key":"old","kind":"string","value":"x"}});
    assert_eq!(
        rpc(&app, token, "tools/call", call.clone()).await["isError"],
        true
    );
    enable(&app, token, &current).await;
    assert_ne!(tool_name(&current, "make"), tool_name(&row, "make"));
    assert_eq!(rpc(&app, token, "tools/call", call).await["isError"], true);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/extensions/{}", row["id"].as_str().unwrap()),
            json!({"version":4,"enabled":false}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        rpc(
            &app,
            token,
            "tools/call",
            json!({"name":tool_name(&current,"search"),"arguments":{}})
        )
        .await["isError"],
        true
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn unknown_bindings_remain_disabled_and_concurrent_catalog_reservations_are_bounded() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut invalid = tools();
    invalid[0]["operation"] = json!("users.becomeOwner");
    let bad = install(&app, token, "unknown-plugin", invalid).await;
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/extensions/{}", bad["id"].as_str().unwrap()),
            json!({"version":1,"enabled":true}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let oversized=Value::Array((0..64).map(|i|json!({"name":format!("wide-{i}"),"description":"Bounded alias","operation":"parameters.list","inputSchema":{"type":"object","properties":{"note":{"type":"string","description":"S".repeat(3700)}}},"mappings":{"search":"note"}})).collect());
    let fat = install(&app, token, "oversized-directory", oversized).await;
    assert_eq!(
        wide_send(
            &app,
            "PATCH",
            &format!("/api/v1/extensions/{}", fat["id"].as_str().unwrap()),
            json!({"version":1,"enabled":true}),
            token
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let many=Value::Array((0..64).map(|i|json!({"name":format!("list-{i}"),"description":"Bounded alias","operation":"parameters.list","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"mappings":{}})).collect());
    let a = install(&app, token, "capacity-a", many.clone()).await;
    let b = install(&app, token, "capacity-b", many).await;
    let pa = format!("/api/v1/extensions/{}", a["id"].as_str().unwrap());
    let pb = format!("/api/v1/extensions/{}", b["id"].as_str().unwrap());
    let (a, b) = tokio::join!(
        app.send(
            "PATCH",
            &pa,
            json!({"version":1,"enabled":true}),
            Some(token)
        ),
        app.send(
            "PATCH",
            &pb,
            json!({"version":1,"enabled":true}),
            Some(token)
        )
    );
    assert!(
        (a.0 == StatusCode::OK && b.0 == StatusCode::CONFLICT)
            || (a.0 == StatusCode::CONFLICT && b.0 == StatusCode::OK),
        "{a:?} {b:?}"
    );
    assert_eq!(
        rpc(&app, token, "tools/list", json!({})).await["tools"]
            .as_array()
            .unwrap()
            .len(),
        89
    );
    app.cleanup().await;
}

#[tokio::test]
async fn token_scopes_and_acceptance_audit_failure_cannot_bypass_business_authority() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = install(&app, token, "token-plugin", tools()).await;
    let path = format!("/api/v1/extensions/{}", row["id"].as_str().unwrap());
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"skills.toggle"}},"validationLevel":"strict"}).await.unwrap();
    assert!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"enabled":true}),
            Some(token)
        )
        .await
        .0
        .is_server_error()
    );
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(token)).await.1["data"]["enabled"],
        false
    );
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{},"validationLevel":"strict"})
        .await
        .unwrap();
    enable(&app, token, &row).await;
    let(status,issued)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Plugin reader","scopes":["mcp.read","mcp.invoke","skills.read","parameters.read"]}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let pat = issued["data"]["secret"].as_str().unwrap();
    let list = rpc(&app, pat, "tools/list", json!({})).await;
    assert!(
        list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == tool_name(&row, "search"))
    );
    assert!(
        !list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == tool_name(&row, "make"))
    );
    let input = json!({"name":tool_name(&row,"make"),"arguments":{"label":"Audit fenced","key":"audit.fenced","kind":"string","value":"safe"}});
    assert_eq!(
        rpc(&app, pat, "tools/call", input.clone()).await["structuredContent"]["error"]["code"],
        "FORBIDDEN"
    );
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"skills.tool.invoke"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        rpc(&app, token, "tools/call", input.clone()).await["isError"],
        true
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{},"validationLevel":"strict"})
        .await
        .unwrap();
    assert_eq!(
        rpc(&app, token, "tools/call", input).await["isError"],
        false
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn role_scoped_catalogs_recheck_the_current_plugin_owner_membership() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap();
    let (owner, owner_role) = account(
        &app,
        admin,
        "plugin-other-owner",
        json!([
            "skills.read",
            "skills.write",
            "mcp.read",
            "mcp.invoke",
            "parameters.read"
        ]),
        "self",
    )
    .await;
    let row = install(&app, &owner, "role-plugin", json!([tools()[0].clone()])).await;
    enable(&app, &owner, &row).await;
    let (viewer, viewer_role) = account(
        &app,
        admin,
        "plugin-role-reader",
        json!(["skills.read", "mcp.read", "mcp.invoke", "parameters.read"]),
        "all",
    )
    .await;
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{viewer_role}"),
            json!({"version":1,"dataScope":"roles","dataScopeRoleIds":[owner_role]}),
            Some(admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        rpc(&app, &viewer, "tools/list", json!({})).await["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == tool_name(&row, "search"))
    );
    let (_, other_role) = account(&app, admin, "plugin-outside-team", json!([]), "self").await;
    let (_, users) = app
        .send(
            "GET",
            "/api/v1/users?pageSize=100&search=plugin-other-owner",
            Value::Null,
            Some(admin),
        )
        .await;
    let user = &users["data"][0];
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{}", user["id"].as_str().unwrap()),
            json!({"version":user["version"],"roleIds":[other_role]}),
            Some(admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        !rpc(&app, &viewer, "tools/list", json!({})).await["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == tool_name(&row, "search"))
    );
    assert_eq!(
        rpc(
            &app,
            &viewer,
            "tools/call",
            json!({"name":tool_name(&row,"search"),"arguments":{}})
        )
        .await["isError"],
        true
    );
    app.cleanup().await;
}
