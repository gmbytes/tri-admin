mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

#[tokio::test]
async fn scope_upgrade_preserves_nested_core_menus_and_repairs_v1_orphans() {
    for already_v1 in [false, true] {
        let app = TestApp::new().await;
        let signed = app.login().await;
        let token = signed["accessToken"].as_str().unwrap();
        let group = ObjectId::new();
        let retired = ObjectId::new();
        let retired_nested = ObjectId::new();
        let kept = ObjectId::new();
        let folder = ObjectId::new();
        let file = ObjectId::new();
        let retired_root = ObjectId::new();
        let role = ObjectId::new();
        let now = DateTime::now();
        let menu = |id: ObjectId,
                    path: &str,
                    view: Option<&str>,
                    parent: Option<ObjectId>,
                    removed: bool| doc! {"_id":id,"label":path,"path":path,"viewKey":view,"permission":null,"parentId":parent,"icon":null,"sort":77,"enabled":!removed,"version":4i64,"createdAt":now,"updatedAt":now,"deletedAt":if removed{mongodb::bson::Bson::DateTime(now)}else{mongodb::bson::Bson::Null}};
        app.database
            .collection::<Document>("menus")
            .insert_many([
                menu(group, "/custom-group", None, None, false),
                menu(
                    retired,
                    "/settings",
                    Some("settings"),
                    Some(group),
                    already_v1,
                ),
                menu(
                    retired_nested,
                    "/dictionaries",
                    Some("dictionaries"),
                    Some(retired),
                    already_v1,
                ),
                menu(
                    kept,
                    "/my-users",
                    Some("users"),
                    Some(retired_nested),
                    false,
                ),
                menu(folder, "/my-folder", None, Some(retired), false),
                menu(file, "/my-files", Some("files"), Some(folder), false),
                menu(
                    retired_root,
                    "/parameters",
                    Some("parameters"),
                    None,
                    already_v1,
                ),
                menu(role, "/my-roles", Some("roles"), Some(retired_root), false),
            ])
            .await
            .unwrap();
        let metadata = app.database.collection::<Document>("_schema_migrations");
        if already_v1 {
            metadata
                .update_one(doc! {"_id":"game-scope"}, doc! {"$set":{"version":1}})
                .await
                .unwrap();
        } else {
            metadata
                .delete_one(doc! {"_id":"game-scope"})
                .await
                .unwrap();
        }
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .unwrap();
        let created=app.send("POST","/api/v1/menus",json!({"label":"New retained menu","path":"/after-upgrade","viewKey":"users","permission":"users.read"}),Some(token)).await;
        assert_eq!(
            created.0,
            StatusCode::CREATED,
            "retained graph must remain editable: {}",
            created.1
        );
        let menus = app.database.collection::<Document>("menus");
        for (id, parent, version) in [
            (kept, Some(group), 5),
            (folder, Some(group), 5),
            (file, Some(folder), 4),
            (role, None, 5),
        ] {
            let row = menus
                .find_one(doc! {"_id":id,"deletedAt":null})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                row.get("parentId"),
                Some(
                    &parent
                        .map(mongodb::bson::Bson::ObjectId)
                        .unwrap_or(mongodb::bson::Bson::Null)
                )
            );
            assert_eq!(row.get_i64("version").unwrap(), version);
            assert_eq!(row.get_i32("sort").unwrap(), 77);
        }
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .unwrap();
        assert_eq!(
            menus
                .find_one(doc! {"_id":kept})
                .await
                .unwrap()
                .unwrap()
                .get_i64("version")
                .unwrap(),
            5
        );
        assert_eq!(
            metadata
                .find_one(doc! {"_id":"game-scope"})
                .await
                .unwrap()
                .unwrap()
                .get_i32("version")
                .unwrap(),
            2
        );
        app.cleanup().await;
    }
}

#[tokio::test]
async fn file_storage_options_are_searchable_without_exposing_configuration_or_secrets() {
    let app = TestApp::new().await;
    let signed = app.login().await;
    let token = signed["accessToken"].as_str().unwrap();
    let (status, options) = app
        .send(
            "GET",
            "/api/v1/files/storage-options?search=local-default",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(options["meta"]["total"], 1);
    let row = &options["data"][0];
    assert_eq!(row["code"], "local-default");
    for field in [
        "configuration",
        "configurationJson",
        "secretBox",
        "credentials",
    ] {
        assert!(row.get(field).is_none());
    }
    assert_eq!(
        app.send("GET", "/api/v1/settings/profiles", Value::Null, Some(token))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    app.cleanup().await;
}

#[tokio::test]
async fn seven_module_upgrade_removes_old_role_grants_and_menus_without_touching_identity_or_data()
{
    let app = TestApp::new().await;
    let signed = app.login().await;
    let token = signed["accessToken"].as_str().unwrap();
    let owner = ObjectId::parse_str(signed["user"]["id"].as_str().unwrap()).unwrap();
    let users = app.database.collection::<Document>("users");
    let original = users.find_one(doc! {"_id":owner}).await.unwrap().unwrap();
    let roles = app.database.collection::<Document>("roles");
    let role = ObjectId::new();
    let now = DateTime::now();
    roles.insert_one(doc!{"_id":role,"name":"Existing role","slug":"existing-role","permissions":["users.read","dictionaries.write","mcp.invoke"],"dataScope":"roles","dataScopeRoleIds":[role],"protected":false,"version":7i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).await.unwrap();
    let menus = app.database.collection::<Document>("menus");
    let retired = ObjectId::new();
    let custom = ObjectId::new();
    menus.insert_many([doc!{"_id":retired,"label":"Retired","path":"/dictionaries","viewKey":"dictionaries","permission":"dictionaries.read","parentId":null,"sort":1000,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null},doc!{"_id":custom,"label":"My users","path":"/my-users","viewKey":"users","permission":"users.read","parentId":null,"sort":1001,"enabled":true,"version":4i64,"createdAt":now,"updatedAt":now,"deletedAt":null}]).await.unwrap();
    let data = ObjectId::new();
    app.database
        .collection::<Document>("dictionaries")
        .insert_one(doc! {"_id":data,"name":"Preserved retired data"})
        .await
        .unwrap();
    let metadata = app.database.collection::<Document>("_schema_migrations");
    metadata
        .delete_one(doc! {"_id":"game-scope"})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let kept = users.find_one(doc! {"_id":owner}).await.unwrap().unwrap();
    assert!(kept.get("passwordHash") == original.get("passwordHash"));
    assert!(kept.get("roleIds") == original.get("roleIds"));
    let role = roles.find_one(doc! {"_id":role}).await.unwrap().unwrap();
    assert_eq!(
        role.get_array("permissions").unwrap(),
        &vec![mongodb::bson::Bson::String("users.read".into())]
    );
    assert_eq!(role.get_str("dataScope").unwrap(), "roles");
    assert_eq!(role.get_i64("version").unwrap(), 8);
    assert!(
        menus
            .find_one(doc! {"_id":retired,"deletedAt":null})
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        menus
            .find_one(doc! {"_id":custom})
            .await
            .unwrap()
            .unwrap()
            .get_i64("version")
            .unwrap(),
        4
    );
    assert_eq!(
        app.database
            .collection::<Document>("dictionaries")
            .count_documents(doc! {"_id":data})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(token))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/auth/me",
            Value::Null,
            Some("tri_at1.retired")
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    metadata
        .update_one(doc! {"_id":"game-scope"}, doc! {"$set":{"version":99}})
        .await
        .unwrap();
    assert!(
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .is_err()
    );
    app.cleanup().await;
}

#[tokio::test]
async fn game_focused_console_has_no_retired_platform_routes_permissions_or_navigation() {
    let app = TestApp::new().await;
    let signed = app.login().await;
    let token = signed["accessToken"].as_str().unwrap();
    let retired = [
        "dictionaries",
        "parameters",
        "settings",
        "data-tools",
        "api-tokens",
        "applications",
        "codegen",
        "extensions",
        "mcp",
        "ai",
        "workflows",
        "versions",
    ];
    for name in retired {
        let path = match name {
            "settings" => "/api/v1/settings/service".to_owned(),
            "mcp" => "/api/v1/mcp/connections".to_owned(),
            "ai" => "/api/v1/ai/providers".to_owned(),
            "workflows" => "/api/v1/workflows/plans".to_owned(),
            "data-tools" => "/api/v1/data-tools/resources".to_owned(),
            _ => format!("/api/v1/{name}"),
        };
        assert_eq!(
            app.send("GET", &path, Value::Null, Some(token)).await.0,
            StatusCode::NOT_FOUND,
            "retired route {path}"
        );
    }
    let (_, permissions) = app
        .send("GET", "/api/v1/permissions", Value::Null, Some(token))
        .await;
    let allowed = [
        "dashboard",
        "users",
        "roles",
        "menus",
        "permissions",
        "audit",
        "files",
    ];
    assert!(
        permissions["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| allowed.contains(&row["group"].as_str().unwrap()))
    );
    let (_, navigation) = app
        .send("GET", "/api/v1/navigation", Value::Null, Some(token))
        .await;
    assert_eq!(navigation["data"].as_array().unwrap().len(), 7);
    let collections = app.database.list_collection_names().await.unwrap();
    for name in [
        "dictionaries",
        "parameters",
        "api_tokens",
        "applications",
        "ai_runs",
        "workflow_runs",
        "versions",
    ] {
        assert!(
            !collections.iter().any(|collection| collection == name),
            "retired collection {name}"
        );
    }
    app.cleanup().await;
}

fn contains_retired_menu(rows: &Value) -> bool {
    rows.as_array().is_some_and(|rows| {
        rows.iter().any(|row| {
            matches!(row["viewKey"].as_str(), Some("customers" | "clients"))
                || contains_retired_menu(&row["children"])
        })
    })
}

#[tokio::test]
async fn customer_and_client_features_have_no_routes_permissions_schema_or_seeded_navigation() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    for (method, path) in [
        ("GET", "/api/v1/customers"),
        ("POST", "/api/v1/customers"),
        ("GET", "/api/v1/clients"),
        ("POST", "/api/v1/clients"),
        ("GET", "/api/v1/customers/owners"),
        ("PATCH", "/api/v1/customers/000000000000000000000000"),
        (
            "DELETE",
            "/api/v1/customers/000000000000000000000000?version=1",
        ),
    ] {
        assert_eq!(
            app.send(method, path, json!({}), Some(token)).await.0,
            StatusCode::NOT_FOUND,
            "{method} {path}"
        );
    }
    let (_, catalog) = app
        .send("GET", "/api/v1/api-catalog", Value::Null, Some(token))
        .await;
    assert!(
        catalog["data"]["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| ["customers", "clients"].iter().all(|resource| {
                !row["operationId"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("{resource}."))
                    && !row["path"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("/api/v1/{resource}"))
            }))
    );
    let (_, permissions) = app
        .send("GET", "/api/v1/permissions", Value::Null, Some(token))
        .await;
    assert!(permissions["data"].as_array().unwrap().iter().all(|row| {
        ["customers.", "clients."]
            .iter()
            .all(|prefix| !row["code"].as_str().unwrap().starts_with(prefix))
    }));
    assert!(!contains_retired_menu(
        &app.send("GET", "/api/v1/navigation", Value::Null, Some(token))
            .await
            .1["data"]
    ));
    let collections = app.database.list_collection_names().await.unwrap();
    for resource in ["customers", "clients"] {
        assert!(!collections.iter().any(|name| name == resource));
        assert_eq!(
            app.database
                .collection::<Document>("_schema_migrations")
                .count_documents(doc! {"_id":resource})
                .await
                .unwrap(),
            0
        );
        assert_eq!(app.send("POST","/api/v1/roles",json!({"name":"retired","slug":"retired","permissions":[format!("{resource}.read")],"dataScope":"self"}),Some(token)).await.0,StatusCode::UNPROCESSABLE_ENTITY);
    }
    app.cleanup().await;
}

#[tokio::test]
async fn stored_unregistered_views_cannot_reintroduce_retired_navigation() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let now = DateTime::now();
    app.database.collection::<Document>("menus").insert_one(doc!{"label":"已移除模块","path":"/retired-customer","viewKey":"customers","icon":null,"permission":null,"parentId":null,"sort":90,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).await.unwrap();
    let (_, navigation) = app
        .send("GET", "/api/v1/navigation", Value::Null, Some(token))
        .await;
    assert!(!contains_retired_menu(&navigation["data"]));
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/menus",
            json!({"label":"retired","path":"/removed-view","viewKey":"customers"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.cleanup().await;
}

#[tokio::test]
async fn inactive_customer_data_does_not_affect_user_deletion_or_readiness_and_is_not_purged() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (_, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":"basic","slug":"basic","permissions":[],"dataScope":"self"}),
            Some(token),
        )
        .await;
    let (_,user)=app.send("POST","/api/v1/users",json!({"username":"retired-owner","displayName":"Retired","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(token)).await;
    let id = user["data"]["id"].as_str().unwrap();
    let record_id = ObjectId::new();
    app.database
        .collection::<Document>("customers")
        .insert_one(
            doc! {"_id":record_id,"ownerId":ObjectId::parse_str(id).unwrap(),"deletedAt":null},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/users/{id}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"customers"}, doc! {"$set":{"version":999}})
        .upsert(true)
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        app.database
            .collection::<Document>("customers")
            .count_documents(doc! {"_id":record_id})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
