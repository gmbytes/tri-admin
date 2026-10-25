mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

async fn role(
    app: &TestApp,
    access: &str,
    slug: &str,
    permissions: Vec<&str>,
    scope: &str,
) -> Value {
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":slug,"slug":slug,"permissions":permissions,"dataScope":scope}),
            Some(access),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}

async fn user(app: &TestApp, access: &str, username: &str, role_id: &str) -> Value {
    let (status,body)=app.send("POST","/api/v1/users",json!({"username":username,"displayName":username,"password":PASSWORD,"roleIds":[role_id]}),Some(access)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}

async fn login_as(app: &TestApp, username: &str, password: &str) -> Value {
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":username,"password":password}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}

#[tokio::test]
async fn owner_manages_users_roles_and_versions_while_viewer_cannot_write() {
    let app = TestApp::new().await;
    let owner = app.login().await;
    let access = owner["accessToken"].as_str().unwrap();
    let viewer_role = role(&app, access, "viewer", vec!["users.read"], "all").await;
    let viewer = user(&app, access, "viewer", viewer_role["id"].as_str().unwrap()).await;
    assert!(viewer.get("passwordHash").is_none());
    assert!(viewer["id"].is_string());
    let tokens = login_as(&app, "viewer", PASSWORD).await;
    let view_access = tokens["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/users", Value::Null, Some(view_access))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("POST", "/api/v1/users", json!({}), Some(view_access))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let path = format!("/api/v1/users/{}", viewer["id"].as_str().unwrap());
    let (status, updated) = app
        .send(
            "PATCH",
            &path,
            json!({"version":1,"displayName":"Updated viewer"}),
            Some(access),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["data"]["version"], 2);
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"displayName":"Stale"}),
            Some(access)
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
            Some(access)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(view_access))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!(
                "/api/v1/roles/{}?version=1",
                viewer_role["id"].as_str().unwrap()
            ),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    app.cleanup().await;
}

#[tokio::test]
async fn final_owner_and_protected_role_cannot_be_removed_or_disabled() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let access = tokens["accessToken"].as_str().unwrap();
    let id = tokens["user"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{id}"),
            json!({"version":1,"status":"disabled"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/users/{id}?version=1"),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let owner_role = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"owner"})
        .await
        .unwrap()
        .unwrap();
    let role_id = owner_role.get_object_id("_id").unwrap().to_hex();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{role_id}"),
            json!({"version":1,"permissions":[]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/roles/{role_id}?version=1"),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_owner_disable_cannot_leave_the_system_without_an_owner() {
    let app = TestApp::new().await;
    let first = app.login().await;
    let first_access = first["accessToken"].as_str().unwrap();
    let owner_role = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"owner"})
        .await
        .unwrap()
        .unwrap();
    let role_id = owner_role.get_object_id("_id").unwrap();
    let second = user(&app, first_access, "owner-two", &role_id.to_hex()).await;
    let second_tokens = login_as(&app, "owner-two", PASSWORD).await;
    let first_path = format!("/api/v1/users/{}", first["user"]["id"].as_str().unwrap());
    let second_path = format!("/api/v1/users/{}", second["id"].as_str().unwrap());
    let (a, b) = tokio::join!(
        app.send(
            "PATCH",
            &first_path,
            json!({"version":1,"status":"disabled"}),
            Some(first_access)
        ),
        app.send(
            "PATCH",
            &second_path,
            json!({"version":1,"status":"disabled"}),
            Some(second_tokens["accessToken"].as_str().unwrap())
        )
    );
    let mut statuses = vec![a.0.as_u16(), b.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, vec![200, 409]);
    assert_eq!(
        app.database
            .collection::<Document>("users")
            .count_documents(doc! {"roleIds":role_id,"status":"active","deletedAt":null})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn delegated_admin_cannot_grant_owner_or_permissions_they_do_not_hold() {
    let app = TestApp::new().await;
    let root = app.login().await;
    let root_access = root["accessToken"].as_str().unwrap();
    let operator_role = role(
        &app,
        root_access,
        "operator",
        vec![
            "users.read",
            "users.write",
            "roles.read",
            "roles.write",
            "roles.assign",
        ],
        "all",
    )
    .await;
    let operator = user(
        &app,
        root_access,
        "operator",
        operator_role["id"].as_str().unwrap(),
    )
    .await;
    let tokens = login_as(&app, "operator", PASSWORD).await;
    let access = tokens["accessToken"].as_str().unwrap();
    let (status,_)=app.send("POST","/api/v1/roles",json!({"name":"escape","slug":"escape","permissions":["settings.write"],"dataScope":"all"}),Some(access)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/roles",
            json!({"name":"wildcard","slug":"wildcard","permissions":["*"],"dataScope":"all"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let owner_role = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"owner"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{}", operator["id"].as_str().unwrap()),
            json!({"version":1,"roleIds":[owner_role.get_object_id("_id").unwrap().to_hex()]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn self_scope_does_not_leak_other_users_through_list_or_detail() {
    let app = TestApp::new().await;
    let root = app.login().await;
    let access = root["accessToken"].as_str().unwrap();
    let own_role = role(&app, access, "self-viewer", vec!["users.read"], "self").await;
    let own = user(
        &app,
        access,
        "self-viewer",
        own_role["id"].as_str().unwrap(),
    )
    .await;
    let tokens = login_as(&app, "self-viewer", PASSWORD).await;
    let own_access = tokens["accessToken"].as_str().unwrap();
    let (status, list) = app
        .send("GET", "/api/v1/users", Value::Null, Some(own_access))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["meta"]["total"], 1);
    assert_eq!(list["data"][0]["id"], own["id"]);
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/users/{}", root["user"]["id"].as_str().unwrap()),
            Value::Null,
            Some(own_access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, filtered) = app
        .send(
            "GET",
            "/api/v1/users?search=admin",
            Value::Null,
            Some(own_access),
        )
        .await;
    assert_eq!(filtered["meta"]["total"], 0);
    app.cleanup().await;
}

#[tokio::test]
async fn validation_uniqueness_unicode_usernames_and_literal_search_are_enforced() {
    let app = TestApp::new().await;
    let root = app.login().await;
    let access = root["accessToken"].as_str().unwrap();
    let role_id = root["user"]["roleIds"][0].as_str().unwrap();
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"admin","displayName":"duplicate","password":PASSWORD,"roleIds":[role_id]}),Some(access)).await.0,StatusCode::CONFLICT);
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"invalid-role","displayName":"test","password":PASSWORD,"roleIds":["000000000000000000000000"]}),Some(access)).await.0,StatusCode::UNPROCESSABLE_ENTITY);
    let username = "测试".repeat(12);
    user(&app, access, &username, role_id).await;
    login_as(&app, &username, PASSWORD).await;
    let (_, list) = app
        .send("GET", "/api/v1/users?search=.*", Value::Null, Some(access))
        .await;
    assert_eq!(list["meta"]["total"], 0);
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/users?pageSize=101",
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.send("GET", "/api/v1/users/not-an-id", Value::Null, Some(access))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.cleanup().await;
}

#[tokio::test]
async fn password_reset_revokes_sessions_and_changes_the_accepted_password() {
    let app = TestApp::new().await;
    let root = app.login().await;
    let access = root["accessToken"].as_str().unwrap();
    let view_role = role(&app, access, "password-viewer", vec!["users.read"], "self").await;
    let account = user(
        &app,
        access,
        "password-user",
        view_role["id"].as_str().unwrap(),
    )
    .await;
    let previous = login_as(&app, "password-user", PASSWORD).await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/users/{}/password", account["id"].as_str().unwrap()),
            json!({"version":1,"password":"NewFixturePassword-2026"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/auth/me",
            Value::Null,
            Some(previous["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"password-user","password":PASSWORD}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    login_as(&app, "password-user", "NewFixturePassword-2026").await;
    app.cleanup().await;
}

fn navigation_paths(value: &Value, paths: &mut Vec<String>) {
    if let Some(items) = value.as_array() {
        for item in items {
            if let Some(path) = item["path"].as_str() {
                paths.push(path.into());
            }
            navigation_paths(&item["children"], paths);
        }
    }
}

#[tokio::test]
async fn menu_cycles_children_and_visibility_are_validated() {
    let app = TestApp::new().await;
    let root = app.login().await;
    let access = root["accessToken"].as_str().unwrap();
    let (status,parent)=app.send("POST","/api/v1/menus",json!({"label":"Custom","path":"/custom","viewKey":null,"parentId":null,"permission":null,"sort":10,"enabled":true}),Some(access)).await;
    assert_eq!(status, StatusCode::CREATED);
    let parent_id = parent["data"]["id"].as_str().unwrap();
    let (status,child)=app.send("POST","/api/v1/menus",json!({"label":"Members","path":"/custom-members","viewKey":"users","parentId":parent_id,"permission":"users.read","sort":1,"enabled":true}),Some(access)).await;
    assert_eq!(status, StatusCode::CREATED);
    let child_id = child["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/menus/{parent_id}"),
            json!({"version":1,"parentId":child_id}),
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/menus/{parent_id}?version=1"),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/menus",
            json!({"label":"unsafe","path":"/unsafe","viewKey":"eval-script","enabled":true}),
            Some(access)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let viewer = role(&app, access, "menu-viewer", vec!["users.read"], "self").await;
    user(&app, access, "menu-viewer", viewer["id"].as_str().unwrap()).await;
    let tokens = login_as(&app, "menu-viewer", PASSWORD).await;
    let view_access = tokens["accessToken"].as_str().unwrap();
    let (status, navigation) = app
        .send("GET", "/api/v1/navigation", Value::Null, Some(view_access))
        .await;
    assert_eq!(status, StatusCode::OK);
    let mut paths = vec![];
    navigation_paths(&navigation["data"], &mut paths);
    assert!(paths.contains(&"/custom-members".into()));
    assert!(!paths.contains(&"/roles".into()));
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/menus/{child_id}"),
            json!({"version":1,"enabled":false}),
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, navigation) = app
        .send("GET", "/api/v1/navigation", Value::Null, Some(view_access))
        .await;
    let mut paths = vec![];
    navigation_paths(&navigation["data"], &mut paths);
    assert!(!paths.contains(&"/custom-members".into()));
    app.cleanup().await;
}

#[tokio::test]
async fn moving_a_menu_subtree_cannot_exceed_the_global_depth_limit() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let access = tokens["accessToken"].as_str().unwrap();
    let mut parent = Value::Null;
    for index in 0..14 {
        let (status,node)=app.send("POST","/api/v1/menus",json!({"label":format!("Depth {index}"),"path":format!("/chain-{index}"),"viewKey":"users","parentId":parent,"permission":"users.read"}),Some(access)).await;
        assert_eq!(status, StatusCode::CREATED);
        parent = node["data"]["id"].clone();
    }
    let mut root = Value::Null;
    let mut branch_parent = Value::Null;
    for index in 0..3 {
        let (status,node)=app.send("POST","/api/v1/menus",json!({"label":format!("Branch {index}"),"path":format!("/branch-{index}"),"viewKey":"users","parentId":branch_parent,"permission":"users.read"}),Some(access)).await;
        assert_eq!(status, StatusCode::CREATED);
        branch_parent = node["data"]["id"].clone();
        if index == 0 {
            root = branch_parent.clone();
        }
    }
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/menus/{}", root.as_str().unwrap()),
            json!({"version":1,"parentId":parent}),
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    app.cleanup().await;
}

#[tokio::test]
async fn explicit_null_parent_moves_a_menu_to_root_without_clearing_omitted_fields() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let access = tokens["accessToken"].as_str().unwrap();
    let (_, parent) = app
        .send(
            "POST",
            "/api/v1/menus",
            json!({"label":"Group","path":"/null-parent","viewKey":null}),
            Some(access),
        )
        .await;
    let (_,child)=app.send("POST","/api/v1/menus",json!({"label":"Child","path":"/null-child","viewKey":"users","parentId":parent["data"]["id"],"permission":"users.read"}),Some(access)).await;
    let (status, moved) = app
        .send(
            "PATCH",
            &format!("/api/v1/menus/{}", child["data"]["id"].as_str().unwrap()),
            json!({"version":1,"parentId":null}),
            Some(access),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(moved["data"]["parentId"].is_null());
    assert_eq!(moved["data"]["viewKey"], "users");
    assert_eq!(
        app.send(
            "DELETE",
            &format!(
                "/api/v1/menus/{}?version=1",
                parent["data"]["id"].as_str().unwrap()
            ),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    app.cleanup().await;
}

#[tokio::test]
async fn requested_sort_precedes_id_and_pagination_is_stable() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let access = tokens["accessToken"].as_str().unwrap();
    let role_id = tokens["user"]["roleIds"][0].as_str().unwrap();
    user(&app, access, "zulu", role_id).await;
    user(&app, access, "alpha", role_id).await;
    let (_, first) = app
        .send(
            "GET",
            "/api/v1/users?sort=username&pageSize=2",
            Value::Null,
            Some(access),
        )
        .await;
    let names = first["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|user| user["username"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["admin", "alpha"]);
    assert_eq!(first["meta"]["total"], 3);
    let (_, second) = app
        .send(
            "GET",
            "/api/v1/users?sort=username&pageSize=2&page=2",
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(second["data"][0]["username"], "zulu");
    app.cleanup().await;
}

#[tokio::test]
async fn rbac_schema_is_required_before_the_application_reports_ready() {
    let app = TestApp::new().await;
    app.database
        .collection::<Document>("_schema_migrations")
        .delete_one(doc! {"_id":"rbac"})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}

#[tokio::test]
async fn menu_paths_cannot_shadow_reserved_client_routes() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let access = tokens["accessToken"].as_str().unwrap();
    for path in ["/", "/login", "/profile"] {
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/menus",
                json!({"label":"Reserved","path":path,"viewKey":"users"}),
                Some(access)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    app.cleanup().await;
}
