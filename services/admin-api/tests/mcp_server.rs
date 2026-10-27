mod support;
use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
use tower::ServiceExt;

const PATH: &str = "/api/v1/mcp/server";
fn config(hosts: &str) -> Result<admin_api::config::Config, admin_api::config::ConfigError> {
    admin_api::config::Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_MCP_ALLOWED_HOSTS" => Some(hosts.into()),
        _ => None,
    })
}
fn message(method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientInfo":{"name":"tri-test","version":"1"},
        "io.modelcontextprotocol/clientCapabilities":{}
    });
    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
}
async fn raw(
    app: &TestApp,
    token: Option<&str>,
    body: Value,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(PATH)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", body["method"].as_str().unwrap_or(""));
    if let Some(name) = body["params"]["name"].as_str() {
        request = request.header("mcp-name", name);
    }
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let mut request = request.body(Body::from(body.to_string())).unwrap();
    for (key, value) in headers {
        request.headers_mut().insert(
            axum::http::HeaderName::from_bytes(key.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    let response = app.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"nonJson":true}))
    };
    (status, headers, value)
}
async fn rpc(app: &TestApp, token: &str, method: &str, params: Value) -> Value {
    let (status, headers, body) = raw(app, Some(token), message(method, params), &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert!(body.get("error").is_none(), "{body}");
    body["result"].clone()
}
async fn call(app: &TestApp, token: &str, name: &str, arguments: Value) -> Value {
    rpc(
        app,
        token,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
    .await
}
async fn identity(
    app: &TestApp,
    admin: &str,
    name: &str,
    permissions: Value,
    scope: &str,
) -> (String, ObjectId) {
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
    let (status, auth) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    (
        auth["data"]["accessToken"].as_str().unwrap().into(),
        ObjectId::parse_str(role_id).unwrap(),
    )
}

#[tokio::test]
async fn modern_http_mcp_runs_real_business_writes_and_keeps_original_audit_actor() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let discovery = rpc(&app, token, "server/discover", json!({})).await;
    assert_eq!(discovery["supportedVersions"], json!(["2026-07-28"]));
    let tools = rpc(&app, token, "tools/list", json!({})).await;
    for name in [
        "parameters.create",
        "dictionaries.items.create",
        "codegen.generations.create",
    ] {
        assert!(
            tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == name),
            "{name}"
        );
    }
    let (status,create_headers,body)=raw(&app,Some(token),message("tools/call",json!({"name":"parameters.create","arguments":{"input":{"name":"MCP参数","key":"mcp.server.param","valueType":"boolean","value":true}}})),&[]).await;
    assert_eq!(status, StatusCode::OK);
    let result = &body["result"];
    assert_eq!(result["isError"], false, "{result}");
    let id = result["structuredContent"]["data"]["id"].as_str().unwrap();
    let (_, parameter) = app
        .send(
            "GET",
            &format!("/api/v1/parameters/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(parameter["data"]["value"], true);
    let changed = call(
        &app,
        token,
        "parameters.update",
        json!({"id":id,"input":{"version":1,"value":false}}),
    )
    .await;
    assert_eq!(changed["structuredContent"]["data"]["version"], 2);
    let stale = call(
        &app,
        token,
        "parameters.update",
        json!({"id":id,"input":{"version":1,"value":true}}),
    )
    .await;
    assert_eq!(stale["isError"], true);
    assert_eq!(stale["structuredContent"]["error"]["code"], "CONFLICT");
    let dictionary = call(
        &app,
        token,
        "dictionaries.create",
        json!({"input":{"name":"MCP字典","key":"mcp.server.dictionary"}}),
    )
    .await;
    let dictionary_id = dictionary["structuredContent"]["data"]["id"]
        .as_str()
        .unwrap();
    let item = call(
        &app,
        token,
        "dictionaries.items.create",
        json!({"id":dictionary_id,"input":{"label":"启用","value":"on"}}),
    )
    .await;
    assert_eq!(item["isError"], false, "{item}");
    let items = call(
        &app,
        token,
        "dictionaries.items.list",
        json!({"id":dictionary_id,"query":{}}),
    )
    .await;
    assert_eq!(items["structuredContent"]["meta"]["total"], 1);
    let actor = ObjectId::parse_str(auth["user"]["id"].as_str().unwrap()).unwrap();
    let event = app
        .database
        .collection::<Document>("audit_events")
        .find_one(doc! {"actorId":actor,"action":"parameters.create"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.get_str("requestId").ok(),
        create_headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
    );
    assert_eq!(app.database.collection::<Document>("audit_events").count_documents(doc!{"actorId":actor,"action":{"$in":["parameters.create","parameters.update","dictionaries.create","dictionaries.items.create"]}}).await.unwrap(),4);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = app.router.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    let response = reqwest::Client::new()
        .post(format!("http://{address}{PATH}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", "parameters.get")
        .body(
            message(
                "tools/call",
                json!({"name":"parameters.get","arguments":{"id":id}}),
            )
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["result"]["structuredContent"]["data"]["value"], false);
    server.abort();
    let _ = server.await;
    app.cleanup().await;
}

#[tokio::test]
async fn business_mcp_filters_tools_and_rechecks_current_role_permissions() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap();
    let (reader, role) = identity(
        &app,
        admin,
        "mcp-reader",
        json!(["mcp.read", "mcp.invoke", "parameters.read"]),
        "self",
    )
    .await;
    let tools = rpc(&app, &reader, "tools/list", json!({})).await;
    let names = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"parameters.list"));
    assert!(!names.contains(&"parameters.create"));
    assert!(!names.iter().any(|name| name.starts_with("codegen.")));
    let denied = call(
        &app,
        &reader,
        "parameters.create",
        json!({"input":{"name":"denied","key":"denied","valueType":"boolean","value":true}}),
    )
    .await;
    assert_eq!(denied["structuredContent"]["error"]["code"], "FORBIDDEN");
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"_id":role},
            doc! {"$set":{"permissions":["mcp.read","mcp.invoke"]}},
        )
        .await
        .unwrap();
    let denied = call(&app, &reader, "parameters.list", json!({})).await;
    assert_eq!(denied["structuredContent"]["error"]["code"], "FORBIDDEN");
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"_id":role},
            doc! {"$set":{"permissions":["mcp.read","parameters.read"]}},
        )
        .await
        .unwrap();
    let denied = call(&app, &reader, "parameters.list", json!({})).await;
    assert_eq!(denied["structuredContent"]["error"]["code"], "FORBIDDEN");
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {"key":"denied"})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn mcp_boundary_rejects_untrusted_origin_legacy_metadata_and_unauthenticated_requests() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let body = message("server/discover", json!({}));
    assert_eq!(
        raw(&app, None, body.clone(), &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        raw(
            &app,
            Some(token),
            body.clone(),
            &[("origin", "https://untrusted.invalid")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        raw(
            &app,
            Some(token),
            body.clone(),
            &[("host", "untrusted.invalid")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, _, error) = raw(
        &app,
        Some(token),
        body.clone(),
        &[("mcp-protocol-version", "2025-03-26")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["jsonrpc"], "2.0", "{error}");
    assert_eq!(error["error"]["code"], -32022);
    assert_eq!(raw(&app,Some(token),message("initialize",json!({"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"test","version":"1"}})),&[]).await.0,StatusCode::BAD_REQUEST);
    let mut missing = message("tools/list", json!({}));
    missing["params"].as_object_mut().unwrap().remove("_meta");
    assert_eq!(
        raw(&app, Some(token), missing, &[]).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        raw(&app, Some(token), body, &[("mcp-method", "tools/call")])
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    app.cleanup().await;
}

#[tokio::test]
async fn codegen_tools_generate_real_artifacts_replay_uuid_and_enforce_owner_scope() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap();
    let (owner, owner_role) = identity(
        &app,
        admin,
        "mcp-code-owner",
        json!(["mcp.read", "mcp.invoke", "codegen.read", "codegen.write"]),
        "self",
    )
    .await;
    let (stranger, _) = identity(
        &app,
        admin,
        "mcp-code-other",
        json!(["mcp.read", "mcp.invoke", "codegen.read", "codegen.write"]),
        "self",
    )
    .await;
    let request = uuid::Uuid::new_v4().to_string();
    let input = json!({"requestId":request,"schema":{"resource":"mcp_inventory","label":"MCP库存","fields":[{"name":"sku","label":"编号","kind":"text","required":true,"unique":true}]}});
    let first = call(&app, &owner, "codegen.definitions.create", input.clone()).await;
    assert_eq!(first["isError"], false, "{first}");
    assert_eq!(first["structuredContent"]["created"], true);
    let definition = first["structuredContent"]["data"]["id"].as_str().unwrap();
    let replay = call(&app, &owner, "codegen.definitions.create", input).await;
    assert_eq!(replay["structuredContent"]["created"], false);
    assert_eq!(replay["structuredContent"]["data"]["id"], definition);
    let denied = call(
        &app,
        &stranger,
        "codegen.definitions.get",
        json!({"id":definition}),
    )
    .await;
    assert_eq!(denied["structuredContent"]["error"]["code"], "NOT_FOUND");
    let hidden = call(
        &app,
        &stranger,
        "codegen.definitions.list",
        json!({"pageSize":1}),
    )
    .await;
    assert_eq!(hidden["structuredContent"]["meta"]["total"], 0);
    let (viewer, viewer_role) = identity(
        &app,
        admin,
        "mcp-code-viewer",
        json!(["mcp.read", "mcp.invoke", "codegen.read"]),
        "self",
    )
    .await;
    let (status, updated) = app
        .send(
            "PATCH",
            &format!("/api/v1/roles/{viewer_role}"),
            json!({"version":1,"dataScope":"roles","dataScopeRoleIds":[owner_role.to_hex()]}),
            Some(admin),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let selected = call(
        &app,
        &viewer,
        "codegen.definitions.list",
        json!({"pageSize":1}),
    )
    .await;
    assert_eq!(selected["structuredContent"]["meta"]["total"], 1);
    assert_eq!(selected["structuredContent"]["data"][0]["id"], definition);
    let request = uuid::Uuid::new_v4().to_string();
    let input = json!({"requestId":request,"definitionId":definition,"definitionVersion":1});
    let generated = call(&app, &owner, "codegen.generations.create", input.clone()).await;
    assert_eq!(generated["isError"], false, "{generated}");
    assert!(
        generated["structuredContent"]["data"]["files"]
            .as_array()
            .unwrap()
            .len()
            > 25
    );
    let id = generated["structuredContent"]["data"]["id"]
        .as_str()
        .unwrap();
    let repeated = call(&app, &owner, "codegen.generations.create", input).await;
    assert_eq!(repeated["structuredContent"]["data"]["id"], id);
    assert_eq!(repeated["structuredContent"]["created"], false);
    let recovered = call(
        &app,
        &owner,
        "codegen.generations.byRequest",
        json!({"requestId":request}),
    )
    .await;
    assert_eq!(recovered["structuredContent"]["data"]["id"], id);
    let preview = call(
        &app,
        &owner,
        "codegen.generations.preview",
        json!({"id":id,"path":"web/src/schema.json"}),
    )
    .await;
    let content: Value = serde_json::from_str(
        preview["structuredContent"]["data"]["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(content["resource"], "mcp_inventory");
    assert_eq!(
        app.database
            .collection::<Document>("code_generations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn mcp_pat_scope_cannot_expand_business_permissions_and_counts_each_request_once() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap();
    let (status,issued)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"MCP reader","scopes":["mcp.read","mcp.invoke","parameters.read"],"minuteLimit":3}),Some(admin)).await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let token = issued["data"]["secret"].as_str().unwrap();
    let tools = rpc(&app, token, "tools/list", json!({})).await;
    assert!(tools["tools"].as_array().unwrap().iter().all(|tool| {
        ["parameters.list", "parameters.get"].contains(&tool["name"].as_str().unwrap())
    }));
    assert_eq!(
        call(&app, token, "parameters.list", json!({})).await["isError"],
        false
    );
    let denied=call(&app,token,"parameters.create",json!({"input":{"name":"secret-marker","key":"mcp.scope.denied","valueType":"boolean","value":true}})).await;
    assert_eq!(denied["structuredContent"]["error"]["code"], "FORBIDDEN");
    assert!(!denied.to_string().contains("secret-marker"));
    assert_eq!(
        raw(&app, Some(token), message("tools/list", json!({})), &[])
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let id = issued["data"]["token"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/api-tokens/{id}/revoke"),
            json!({}),
            Some(admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        raw(&app, Some(token), message("tools/list", json!({})), &[])
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn explicit_mcp_host_allowlist_enables_remote_deployment_without_wildcards() {
    let mut app = TestApp::new().await;
    app.router = admin_api::build_router(admin_api::state::AppState::new(
        config("admin.example.test:8443").unwrap(),
        Some(app.database.clone()),
    ));
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (status, _, body) = raw(
        &app,
        Some(token),
        message("server/discover", json!({})),
        &[("host", "admin.example.test:8443")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for host in [
        "admin.example.test:8444",
        "other.example.test:8443",
        "localhost",
    ] {
        assert_eq!(
            raw(
                &app,
                Some(token),
                message("server/discover", json!({})),
                &[("host", host)]
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    for invalid in [
        "*",
        "*.example.test",
        "https://admin.example.test",
        "user@admin.example.test",
        "admin.example.test/path",
        "",
    ] {
        assert!(config(invalid).is_err(), "{invalid}");
    }
    app.cleanup().await;
}
