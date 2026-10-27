mod support;
use admin_api::{build_router, config::Config, state::AppState};
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::TryStreamExt;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use std::time::Duration;
use support::{PASSWORD, TestApp};
use tempfile::TempDir;
use tri_extension_package::package::{self, File};
fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}
async fn fixture() -> (TestApp, AppState, TempDir) {
    let mut app = TestApp::new().await;
    let temp = TempDir::new().unwrap();
    let environment = json!({"TRI_FIXTURE_ALLOWED":"approved-subset-marker"});
    #[cfg(windows)]
    let environment = {
        let mut environment = environment;
        environment["SystemRoot"] = json!(std::env::var("SystemRoot").unwrap());
        environment
    };
    let file = json!({"formatVersion":1,"programs":[{"id":"records","label":"Approved records","program":env!("CARGO_BIN_EXE_tri-admin-mcp-test-peer"),"arguments":["normal",temp.path().to_string_lossy()],"workingDirectory":temp.path(),"environment":environment,"allowedTools":["increment","inspect","effect_then_wait"],"startupTimeoutMs":2000,"requestTimeoutMs":5000,"stopTimeoutMs":3000}]});
    let path = temp.path().join("programs.json");
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let config = Config::from_lookup(|name| match name {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
        "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
        "TRI_MCP_PROGRAMS_FILE" => Some(path.to_string_lossy().into()),
        _ => None,
    })
    .unwrap();
    let state = AppState::new(config, Some(app.database.clone()));
    app.router = build_router(state.clone());
    (app, state, temp)
}
fn archive(name: &str, capability: &str, tools: Value) -> String {
    let manifest = json!({"formatVersion":1,"name":name,"version":"1.0.0","description":"Approved process fixture","skills":[],"tools":[],"mcp":[{"id":"records","label":"Plugin records","capabilityId":capability,"tools":tools}]});
    STANDARD.encode(
        package::repack(
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
        .unwrap()
        .archive,
    )
}
async fn install(app: &TestApp, token: &str, name: &str, capability: &str, tools: Value) -> Value {
    let(status,body)=app.send("POST","/api/v1/extensions",json!({"requestId":uuid(),"source":{"kind":"archive","contentBase64":archive(name,capability,tools)}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}
async fn toggle(
    app: &TestApp,
    token: &str,
    id: &str,
    version: i64,
    enabled: bool,
) -> (StatusCode, Value) {
    app.send(
        "PATCH",
        &format!("/api/v1/extensions/{id}"),
        json!({"version":version,"enabled":enabled}),
        Some(token),
    )
    .await
}
fn source(row: &Value) -> Value {
    json!({"kind":"plugin","extensionId":row["id"],"revisionId":row["revisionId"],"contributionId":"records"})
}
async fn connection(app: &TestApp, token: &str, transport: Value) -> Value {
    let(status,body)=app.send("POST","/api/v1/mcp/connections",json!({"requestId":uuid(),"name":format!("Plugin process {}",uuid()),"enabled":true,"transport":transport}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}
async fn action(app: &TestApp, token: &str, id: &str, version: i64, kind: &str) -> Value {
    let (status, body) = app
        .send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            json!({"requestId":uuid(),"version":version,"kind":kind}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body["data"].clone()
}
async fn settled(app: &TestApp, token: &str, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (status, body) = app
                .send(
                    "GET",
                    &format!("/api/v1/mcp/actions/{id}"),
                    Value::Null,
                    Some(token),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if !["pending", "running"].contains(&body["data"]["status"].as_str().unwrap()) {
                return body["data"].clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await
        }
    })
    .await
    .unwrap()
}
async fn current(app: &TestApp, token: &str, id: &str) -> Value {
    let (status, body) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}
async fn released(app: &TestApp, token: &str, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            let row = current(app, token, id).await;
            if row["runtime"]["releaseConfirmed"] == true {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(30)).await
        }
    })
    .await
    .unwrap()
}
async fn call(
    app: &TestApp,
    token: &str,
    id: &str,
    version: i64,
    name: &str,
    request: &str,
) -> (StatusCode, Value) {
    app.send(
        "POST",
        &format!("/api/v1/mcp/connections/{id}/calls"),
        json!({"requestId":request,"version":version,"name":name,"arguments":{}}),
        Some(token),
    )
    .await
}
async fn payload(app: &TestApp, token: &str, id: &str) -> Value {
    let (status, body) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/actions/{id}/result"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str(body["data"]["content"].as_str().unwrap()).unwrap()
}
fn alive(pid: i64) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command",&format!("if(Get-Process -Id {pid} -ErrorAction SilentlyContinue){{exit 0}}else{{exit 1}}")]).creation_flags(0x08000000).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap().success()
    }
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }
}

#[tokio::test]
async fn approved_plugin_source_runs_only_its_subset_and_reenable_fences_the_old_instance() {
    let (app, _state, temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let package = install(
        &app,
        token,
        "process-plugin",
        "records",
        json!(["increment", "inspect"]),
    )
    .await;
    let extension = package["id"].as_str().unwrap();
    let (status, body) = toggle(&app, token, extension, 1, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!temp.path().join("methods.txt").exists());
    let row = connection(&app, token, source(&package)).await;
    let id = row["id"].as_str().unwrap();
    assert_eq!(row["runtime"], Value::Null);
    assert!(!temp.path().join("methods.txt").exists());
    let started = action(&app, token, id, 1, "start").await;
    assert_eq!(
        settled(&app, token, started["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let running = current(&app, token, id).await;
    assert_eq!(running["runtime"]["transportKind"], "stdio");
    let pid = running["runtime"]["pid"].as_i64().unwrap();
    assert!(alive(pid));
    let (_, tools) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}/tools"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(
        tools["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["increment", "inspect"]
    );
    assert_eq!(
        call(&app, token, id, 2, "effect_then_wait", &uuid())
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let request = uuid();
    let (status, first) = call(&app, token, id, 2, "increment", &request).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    let called = first["data"]["id"].as_str().unwrap();
    assert_eq!(settled(&app, token, called).await["status"], "succeeded");
    let output = payload(&app, token, called).await;
    assert_eq!(output["structuredContent"]["value"], 1);
    assert_eq!(output["structuredContent"]["hasParentEnvironment"], false);
    let (status, replay) = call(&app, token, id, 2, "increment", &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["id"], called);
    assert_eq!(
        toggle(&app, token, extension, 2, false).await.0,
        StatusCode::OK
    );
    assert_eq!(
        toggle(&app, token, extension, 3, true).await.0,
        StatusCode::OK
    );
    let stopped = released(&app, token, id).await;
    assert!(!alive(pid));
    assert_eq!(stopped["capabilityCurrent"], false);
    assert_eq!(stopped["runtime"]["transportKind"], "stdio");
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/mcp/connections/{id}/actions"),
            json!({"requestId":uuid(),"version":2,"kind":"start"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, replay) = call(&app, token, id, 2, "increment", &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["id"], called);
    assert_eq!(
        payload(&app, token, called).await["structuredContent"]["value"],
        1
    );
    let(status,body)=app.send("PATCH",&format!("/api/v1/mcp/connections/{id}"),json!({"version":2,"name":"Reconfirmed plugin process","enabled":true,"transport":source(&package)}),Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let again = action(&app, token, id, 3, "start").await;
    assert_eq!(
        settled(&app, token, again["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    assert_ne!(current(&app, token, id).await["runtime"]["pid"], pid);
    let stop = action(&app, token, id, 4, "stop").await;
    assert_eq!(
        settled(&app, token, stop["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn changing_plugin_revision_stops_owned_process_and_keeps_sent_effect_unknown() {
    let (app, _state, temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let package = install(
        &app,
        token,
        "revision-process",
        "records",
        json!(["effect_then_wait", "inspect"]),
    )
    .await;
    let extension = package["id"].as_str().unwrap();
    assert_eq!(
        toggle(&app, token, extension, 1, true).await.0,
        StatusCode::OK
    );
    let row = connection(&app, token, source(&package)).await;
    let id = row["id"].as_str().unwrap();
    let start = action(&app, token, id, 1, "start").await;
    assert_eq!(
        settled(&app, token, start["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let pid = current(&app, token, id).await["runtime"]["pid"]
        .as_i64()
        .unwrap();
    let request = uuid();
    let (status, waiting) = call(&app, token, id, 2, "effect_then_wait", &request).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !std::fs::read_to_string(temp.path().join("methods.txt"))
            .unwrap_or_default()
            .contains("tools/call")
        {
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap();
    let (status, inspect) = call(&app, token, id, 2, "inspect", &uuid()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(
        settled(&app, token, inspect["data"]["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    assert_eq!(
        payload(&app, token, inspect["data"]["id"].as_str().unwrap()).await["structuredContent"]["value"],
        1
    );
    let(status,revised)=app.send("POST",&format!("/api/v1/extensions/{extension}/revisions"),json!({"requestId":uuid(),"version":2,"source":{"kind":"archive","contentBase64":archive("revision-process","records",json!(["inspect"]))}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{revised}");
    let updated = revised["data"].clone();
    released(&app, token, id).await;
    assert!(!alive(pid));
    let old = waiting["data"]["id"].as_str().unwrap();
    assert_eq!(settled(&app, token, old).await["status"], "unknown");
    let (status, replayed) = call(&app, token, id, 2, "effect_then_wait", &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["data"]["id"], old);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("methods.txt"))
            .unwrap()
            .lines()
            .filter(|v| *v == "tools/call")
            .count(),
        2
    );
    assert_eq!(
        toggle(&app, token, extension, 3, true).await.0,
        StatusCode::OK
    );
    let (status, body) = app
        .send(
            "PATCH",
            &format!("/api/v1/mcp/connections/{id}"),
            json!({"version":2,"name":"New revision","enabled":true,"transport":source(&updated)}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let start = action(&app, token, id, 3, "start").await;
    assert_eq!(
        settled(&app, token, start["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    assert_eq!(
        call(&app, token, id, 4, "effect_then_wait", &uuid())
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let stop = action(&app, token, id, 4, "stop").await;
    settled(&app, token, stop["id"].as_str().unwrap()).await;
    app.cleanup().await;
}

#[tokio::test]
async fn unknown_programs_or_tool_expansion_cannot_be_activated() {
    let (app, _state, temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    for (name, capability, tools) in [
        ("unknown-program", "missing", json!(["inspect"])),
        ("expanded-program", "records", json!(["not_approved"])),
    ] {
        let package = install(&app, token, name, capability, tools).await;
        let (status, body) = toggle(&app, token, package["id"].as_str().unwrap(), 1, true).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    assert!(!temp.path().join("methods.txt").exists());
    app.cleanup().await;
}

#[tokio::test]
async fn plugin_sources_require_current_skill_scope_but_cleanup_does_not() {
    let (app, _state, _temp) = fixture().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let package = install(&app, owner, "scope-program", "records", json!(["inspect"])).await;
    assert_eq!(
        toggle(&app, owner, package["id"].as_str().unwrap(), 1, true)
            .await
            .0,
        StatusCode::OK
    );
    let(status,role)=app.send("POST","/api/v1/roles",json!({"name":"Source operator","slug":"source-operator","permissions":["mcp.read","mcp.write","mcp.invoke","skills.read"],"dataScope":"all"}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED);
    let role = role["data"]["id"].as_str().unwrap();
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"source-operator","displayName":"Source operator","password":PASSWORD,"roleIds":[role]}),Some(owner)).await.0,StatusCode::CREATED);
    let (_, signed) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"source-operator","password":PASSWORD}),
            None,
        )
        .await;
    let token = signed["data"]["accessToken"].as_str().unwrap();
    let row = connection(&app, token, source(&package)).await;
    let id = row["id"].as_str().unwrap();
    let start = action(&app, token, id, 1, "start").await;
    assert_eq!(
        settled(&app, token, start["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    assert_eq!(app.send("PATCH",&format!("/api/v1/roles/{role}"),json!({"version":1,"dataScope":"self","permissions":["mcp.read","mcp.write","mcp.invoke","skills.read"]}),Some(owner)).await.0,StatusCode::OK);
    assert_eq!(
        call(&app, token, id, 2, "inspect", &uuid()).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{role}"),
            json!({"version":2,"permissions":["mcp.read","mcp.write","mcp.invoke"]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, token, id, 2, "inspect", &uuid()).await.0,
        StatusCode::FORBIDDEN
    );
    let stop = action(&app, token, id, 2, "stop").await;
    assert_eq!(
        settled(&app, token, stop["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn own_mcp3_upgrade_preserves_existing_connections_and_fences_old_readiness() {
    assert_eq!(admin_api::storage::mcp_schema::VERSION, 4);
    let (app, _state, _temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    connection(
        &app,
        token,
        json!({"kind":"stdio","capabilityId":"records"}),
    )
    .await;
    let(status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":"Preserved HTTP","code":"preserved-http","enabled":true,"configuration":{"kind":"http","baseUrl":"http://127.0.0.1:9/mcp"}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    connection(&app,token,json!({"kind":"http","profileId":profile["data"]["id"],"profileVersion":1,"allowedTools":["inspect"]})).await;
    let before = app
        .database
        .collection::<Document>("mcp_connections")
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"mcp"}, doc! {"$set":{"version":3}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("mcp_connections")
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    app.cleanup().await;
}

async fn account(
    app: &TestApp,
    admin: &str,
    name: &str,
    permissions: Value,
    scope: &str,
    scope_roles: Value,
) -> (String, String, String) {
    let(status,role)=app.send("POST","/api/v1/roles",json!({"name":name,"slug":name,"permissions":permissions,"dataScope":scope,"dataScopeRoleIds":scope_roles}),Some(admin)).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let role = role["data"]["id"].as_str().unwrap();
    let (status, user) = app
        .send(
            "POST",
            "/api/v1/users",
            json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role]}),
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
        role.into(),
        user["data"]["id"].as_str().unwrap().into(),
    )
}

#[tokio::test]
async fn source_directory_filters_current_scope_before_count_and_paging_without_exposing_commands()
{
    let (app, _state, temp) = fixture().await;
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap();
    let (owner, owner_role, owner_id) = account(
        &app,
        admin,
        "directory-owner",
        json!(["skills.read", "skills.write", "mcp.read"]),
        "self",
        json!([]),
    )
    .await;
    let mut packages = Vec::new();
    for name in ["directory-a", "directory-b", "directory-c"] {
        let package = install(&app, &owner, name, "records", json!(["inspect"])).await;
        assert_eq!(
            toggle(&app, &owner, package["id"].as_str().unwrap(), 1, true)
                .await
                .0,
            StatusCode::OK
        );
        packages.push(package);
    }
    let (status, first) = app
        .send(
            "GET",
            "/api/v1/mcp/plugin-capabilities?page=1&pageSize=1&sort=name&search=directory-",
            Value::Null,
            Some(&owner),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["meta"]["total"], 3);
    assert_eq!(first["data"][0]["extensionId"], packages[0]["id"]);
    assert_eq!(first["data"][0]["revisionId"], packages[0]["revisionId"]);
    assert_eq!(first["data"][0]["extensionVersion"], 2);
    assert_eq!(first["data"][0]["allowedTools"], json!(["inspect"]));
    assert_eq!(first["data"][0]["current"], true);
    assert!(!first.to_string().contains("tri-admin-mcp-test-peer"));
    assert!(!first.to_string().contains("approved-subset-marker"));
    assert!(first["data"][0].get("program").is_none());
    assert!(first["data"][0].get("environment").is_none());
    let (_, second) = app
        .send(
            "GET",
            "/api/v1/mcp/plugin-capabilities?page=2&pageSize=1&sort=name",
            Value::Null,
            Some(&owner),
        )
        .await;
    assert_eq!(second["data"][0]["extensionId"], packages[1]["id"]);
    let (reader, _, _) = account(
        &app,
        admin,
        "directory-self",
        json!(["mcp.read", "skills.read"]),
        "self",
        json!([]),
    )
    .await;
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/mcp/plugin-capabilities?pageSize=1",
            Value::Null,
            Some(&reader)
        )
        .await
        .1["meta"]["total"],
        0
    );
    let (denied, _, _) = account(
        &app,
        admin,
        "directory-denied",
        json!(["mcp.read"]),
        "all",
        json!([]),
    )
    .await;
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/mcp/plugin-capabilities",
            Value::Null,
            Some(&denied)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (viewer, _, _) = account(
        &app,
        admin,
        "directory-roles",
        json!(["mcp.read", "skills.read"]),
        "roles",
        json!([owner_role]),
    )
    .await;
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/mcp/plugin-capabilities?pageSize=1",
            Value::Null,
            Some(&viewer)
        )
        .await
        .1["meta"]["total"],
        3
    );
    let (_, outside, _) = account(
        &app,
        admin,
        "directory-outside",
        json!([]),
        "self",
        json!([]),
    )
    .await;
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{owner_id}"),
            json!({"version":1,"roleIds":[outside]}),
            Some(admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/mcp/plugin-capabilities?pageSize=1",
            Value::Null,
            Some(&viewer)
        )
        .await
        .1["meta"]["total"],
        0
    );
    assert!(!temp.path().join("methods.txt").exists());
    app.cleanup().await;
}

#[tokio::test]
async fn token_scope_and_strict_source_inputs_are_checked_before_process_creation() {
    let (app, _state, temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let package = install(&app, token, "token-source", "records", json!(["inspect"])).await;
    assert_eq!(
        toggle(&app, token, package["id"].as_str().unwrap(), 1, true)
            .await
            .0,
        StatusCode::OK
    );
    for (name, scopes, expected) in [
        (
            "limited-source",
            json!(["mcp.read", "mcp.write", "mcp.invoke"]),
            StatusCode::FORBIDDEN,
        ),
        (
            "allowed-source",
            json!(["mcp.read", "mcp.write", "mcp.invoke", "skills.read"]),
            StatusCode::CREATED,
        ),
    ] {
        let (status, issued) = app
            .send(
                "POST",
                "/api/v1/api-tokens",
                json!({"requestId":uuid(),"name":name,"scopes":scopes}),
                Some(token),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let pat = issued["data"]["secret"].as_str().unwrap();
        assert_eq!(app.send("POST","/api/v1/mcp/connections",json!({"requestId":uuid(),"name":"Token source","enabled":true,"transport":source(&package)}),Some(pat)).await.0,expected);
    }
    let mut invalid = source(&package);
    invalid["program"] = json!("unapproved.exe");
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/mcp/connections",
            json!({"requestId":uuid(),"name":"Invalid source","enabled":true,"transport":invalid}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut invalid = source(&package);
    invalid["revisionId"] = json!(mongodb::bson::oid::ObjectId::new().to_hex());
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/mcp/connections",
            json!({"requestId":uuid(),"name":"Old source","enabled":true,"transport":invalid}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(!temp.path().join("methods.txt").exists());
    app.cleanup().await;
}

#[tokio::test]
async fn an_empty_declared_subset_never_means_all_program_tools() {
    let (app, _state, _temp) = fixture().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let package = install(&app, token, "empty-source", "records", json!([])).await;
    assert_eq!(
        toggle(&app, token, package["id"].as_str().unwrap(), 1, true)
            .await
            .0,
        StatusCode::OK
    );
    let row = connection(&app, token, source(&package)).await;
    let id = row["id"].as_str().unwrap();
    let start = action(&app, token, id, 1, "start").await;
    assert_eq!(
        settled(&app, token, start["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let (status, tools) = app
        .send(
            "GET",
            &format!("/api/v1/mcp/connections/{id}/tools"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tools["data"], json!([]));
    assert_eq!(
        call(&app, token, id, 2, "inspect", &uuid()).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let stop = action(&app, token, id, 2, "stop").await;
    assert_eq!(
        settled(&app, token, stop["id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn mcp3_migration_does_not_adopt_unsupported_plugin_records() {
    let (app, _state, _temp) = fixture().await;
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"mcp"}, doc! {"$set":{"version":3}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("mcp_connections")
        .insert_one(doc! {"transport":{"kind":"plugin"}})
        .await
        .unwrap();
    assert!(
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .is_err()
    );
    assert_eq!(
        app.database
            .collection::<Document>("_schema_migrations")
            .find_one(doc! {"_id":"mcp"})
            .await
            .unwrap()
            .unwrap()
            .get_i32("version")
            .unwrap(),
        3
    );
    app.cleanup().await;
}
