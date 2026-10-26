mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

#[tokio::test]
async fn token_scope_choices_need_only_issuance_permission_and_never_offer_session_only_writes() {
    let app = TestApp::new().await;
    let owner = app.login().await;
    let token = owner["accessToken"].as_str().unwrap();
    let (_, role) = app.send("POST", "/api/v1/roles", json!({"name":"Issuer","slug":"issuer","permissions":["apiTokens.read","apiTokens.write","parameters.read"],"dataScope":"self"}), Some(token)).await;
    let (status, _) = app.send("POST", "/api/v1/users", json!({"username":"issuer","displayName":"Issuer","password":PASSWORD,"roleIds":[role["data"]["id"]]}), Some(token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, auth) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"issuer","password":PASSWORD}),
            None,
        )
        .await;
    let issuer = auth["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/permissions", Value::Null, Some(issuer))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, choices) = app
        .send(
            "GET",
            "/api/v1/api-tokens/scopes",
            Value::Null,
            Some(issuer),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let codes = choices["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["code"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(codes, ["apiTokens.read", "parameters.read"]);
    let (_, all) = app
        .send("GET", "/api/v1/api-tokens/scopes", Value::Null, Some(token))
        .await;
    for forbidden in [
        "*",
        "apiTokens.write",
        "users.write",
        "applications.write",
        "applicationUsers.write",
        "gameServers.write",
    ] {
        assert!(
            !all["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["code"] == forbidden)
        );
    }
    let (_, issue) = app.send("POST", "/api/v1/api-tokens", json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"No credential chaining","scopes":["apiTokens.write"]}), Some(token)).await;
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/api-tokens/scopes",
            Value::Null,
            issue["data"]["secret"].as_str()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn own_v4_menu_upgrade_preserves_occupied_paths_customization_and_deleted_entries() {
    let app = TestApp::new().await;
    let menus = app.database.collection::<Document>("menus");
    menus
        .delete_many(doc! {"path":{"$in":["/api-tokens","/applications"]}})
        .await
        .unwrap();
    let now = DateTime::now();
    menus.insert_one(doc! {"label":"My occupied route","path":"/applications","viewKey":"dashboard","permission":null,"parentId":null,"sort":42,"enabled":true,"version":7i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).await.unwrap();
    let metadata = app.database.collection::<Document>("_schema_migrations");
    metadata
        .update_one(doc! {"_id":"open-access"}, doc! {"$set":{"version":4}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let token_menu = menus
        .find_one(doc! {"path":"/api-tokens"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(token_menu.get_str("viewKey").unwrap(), "tokens");
    assert_eq!(
        menus
            .find_one(doc! {"path":"/applications"})
            .await
            .unwrap()
            .unwrap()
            .get_str("viewKey")
            .unwrap(),
        "dashboard"
    );
    menus
        .update_one(
            doc! {"_id":token_menu.get_object_id("_id").unwrap()},
            doc! {"$set":{"label":"Customized token menu","enabled":false,"deletedAt":now}},
        )
        .await
        .unwrap();
    metadata
        .update_one(doc! {"_id":"open-access"}, doc! {"$set":{"version":4}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let retained = menus
        .find_one(doc! {"_id":token_menu.get_object_id("_id").unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.get_str("label").unwrap(), "Customized token menu");
    assert!(!retained.get_bool("enabled").unwrap());
    assert_eq!(retained.get_datetime("deletedAt").unwrap(), &now);
    assert_eq!(
        menus
            .count_documents(doc! {"path":"/api-tokens"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        metadata
            .find_one(doc! {"_id":"open-access"})
            .await
            .unwrap()
            .unwrap()
            .get_i32("version")
            .unwrap(),
        5
    );
    app.cleanup().await;
}
