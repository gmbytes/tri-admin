mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

#[path = "support/config.rs"]
mod config;
use config::bundle;
async fn preview(app: &TestApp, token: &str, bundle: Value, strategy: &str) -> Value {
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/versions/import-previews",
            json!({"bundle":bundle,"strategy":strategy}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}
async fn identity(app: &TestApp, owner: &str, codes: Value) -> String {
    let (status,role)=app.send("POST","/api/v1/roles",json!({"name":"preview-actor","slug":"preview-actor","permissions":codes,"dataScope":"all"}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let (status,user)=app.send("POST","/api/v1/users",json!({"username":"preview-actor","displayName":"Preview","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"preview-actor","password":PASSWORD}),
            None,
        )
        .await;
    login["data"]["accessToken"].as_str().unwrap().into()
}

#[tokio::test]
async fn import_preview_reports_changes_without_mutating_configuration_and_binds_actor_state_and_expiry()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let menus = app
        .database
        .collection::<Document>("menus")
        .count_documents(doc! {"deletedAt":null})
        .await
        .unwrap();
    let result = preview(&app, token, bundle(), "createOnly").await;
    assert_eq!(result["canApply"], true);
    assert_eq!(result["counts"]["menus"]["create"], 2);
    assert_eq!(result["counts"]["dictionaries"]["create"], 1);
    assert_eq!(result["counts"]["dictionaryItems"]["create"], 1);
    assert_eq!(result["counts"]["parameters"]["create"], 1);
    assert_eq!(result["checksum"].as_str().unwrap().len(), 64);
    assert!(result["expiresAt"].as_str().unwrap().ends_with('Z'));
    assert_eq!(
        app.database
            .collection::<Document>("menus")
            .count_documents(doc! {"deletedAt":null})
            .await
            .unwrap(),
        menus
    );
    for collection in ["dictionaries", "parameters", "versions"] {
        assert_eq!(
            app.database
                .collection::<Document>(collection)
                .count_documents(doc! {"deletedAt":null})
                .await
                .unwrap(),
            0
        );
    }
    let id = mongodb::bson::oid::ObjectId::parse_str(result["id"].as_str().unwrap()).unwrap();
    let stored = app
        .database
        .collection::<Document>("version_previews")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.get_object_id("actorId").unwrap().to_hex(),
        auth["user"]["id"]
    );
    assert_eq!(stored.get_str("status").unwrap(), "pending");
    assert_eq!(stored.get_str("baselineHash").unwrap().len(), 64);
    assert!(
        stored.get_datetime("expiresAt").unwrap().timestamp_millis()
            > mongodb::bson::DateTime::now().timestamp_millis()
    );
    app.cleanup().await;
}

#[tokio::test]
async fn import_preview_rejects_wrong_formats_schema_unknown_interfaces_duplicates_and_invalid_domain_fields()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let original = bundle();
    let mut cases = vec![];
    let mut value = original.clone();
    value["format"] = json!("old-package");
    cases.push(value);
    let mut value = original.clone();
    value["schemaVersion"] = json!(99);
    cases.push(value);
    let mut value = original.clone();
    value["requiredApis"][0]["operationId"] = json!("missing.api");
    cases.push(value);
    let mut value = original.clone();
    value["requiredApis"][0]["method"] = json!("POST");
    cases.push(value);
    let mut value = original.clone();
    value["resources"]["menus"][0]["viewKey"] = json!("customers");
    cases.push(value);
    let mut value = original.clone();
    value["resources"]["menus"][0]["path"] = json!("/login");
    cases.push(value);
    let mut value = original.clone();
    value["resources"]["menus"][0]["path"] = value["resources"]["menus"][1]["path"].clone();
    cases.push(value);
    let mut value = original.clone();
    value["resources"]["dictionaries"][0]["items"]
        .as_array_mut()
        .unwrap()
        .push(original["resources"]["dictionaries"][0]["items"][0].clone());
    cases.push(value);
    let mut value = original.clone();
    value["resources"]["parameters"][0]["value"] = json!("false");
    cases.push(value);
    let mut value = original.clone();
    value["release"]["capturedAt"] = json!("invalid-time");
    cases.push(value);
    for value in cases {
        let (status, body) = app
            .send(
                "POST",
                "/api/v1/versions/import-previews",
                json!({"bundle":value}),
                Some(token),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    assert_eq!(
        app.database
            .collection::<Document>("version_previews")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn create_only_conflicts_are_explicit_and_merge_is_an_explicit_preview_choice() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (status, _) = app
        .send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"开关","key":"import.enabled","valueType":"boolean","value":true}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let result = preview(&app, token, bundle(), "createOnly").await;
    assert_eq!(result["canApply"], false);
    assert_eq!(result["counts"]["parameters"]["conflict"], 1);
    let result = preview(&app, token, bundle(), "merge").await;
    assert_eq!(result["canApply"], true);
    assert_eq!(result["counts"]["parameters"]["update"], 1);
    let current = app
        .database
        .collection::<Document>("parameters")
        .find_one(doc! {"key":"import.enabled"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.get_str("valueJson").unwrap(), "true");
    app.cleanup().await;
}

#[tokio::test]
async fn preview_needs_resource_read_access_and_reports_missing_write_access_without_escalation() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let token = identity(
        &app,
        owner,
        json!([
            "versions.write",
            "menus.read",
            "dictionaries.read",
            "parameters.read",
            "permissions.read"
        ]),
    )
    .await;
    let result = preview(&app, &token, bundle(), "merge").await;
    assert_eq!(result["canApply"], false);
    assert_eq!(
        result["missingWritePermissions"].as_array().unwrap().len(),
        3
    );
    let id = mongodb::bson::oid::ObjectId::parse_str(
        app.database
            .collection::<Document>("roles")
            .find_one(doc! {"slug":"preview-actor"})
            .await
            .unwrap()
            .unwrap()
            .get_object_id("_id")
            .unwrap()
            .to_hex(),
    )
    .unwrap();
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"permissions":["versions.write"]}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/versions/import-previews",
            json!({"bundle":bundle()}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn preview_validates_the_complete_proposed_menu_graph_including_existing_subtrees() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut value = bundle();
    value["resources"]["menus"][1]["parentPath"] = json!("/import-child");
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/versions/import-previews",
            json!({"bundle":value}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut value = bundle();
    value["resources"]["menus"][0]["parentPath"] = json!("/missing-parent");
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/versions/import-previews",
            json!({"bundle":value}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.cleanup().await;
}

#[tokio::test]
async fn previews_remember_target_versions_even_when_the_target_value_is_unchanged() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (_, parameter) = app
        .send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"开关","key":"import.enabled","valueType":"boolean","value":false}),
            Some(token),
        )
        .await;
    let first = preview(&app, token, bundle(), "merge").await;
    app.send(
        "PATCH",
        &format!(
            "/api/v1/parameters/{}",
            parameter["data"]["id"].as_str().unwrap()
        ),
        json!({"version":1,"value":false}),
        Some(token),
    )
    .await;
    let second = preview(&app, token, bundle(), "merge").await;
    let docs = app.database.collection::<Document>("version_previews");
    let one=docs.find_one(doc!{"_id":mongodb::bson::oid::ObjectId::parse_str(first["id"].as_str().unwrap()).unwrap()}).await.unwrap().unwrap();
    let two=docs.find_one(doc!{"_id":mongodb::bson::oid::ObjectId::parse_str(second["id"].as_str().unwrap()).unwrap()}).await.unwrap().unwrap();
    assert_ne!(
        one.get_str("baselineHash").unwrap(),
        two.get_str("baselineHash").unwrap()
    );
    assert_eq!(second["counts"]["parameters"]["unchanged"], 1);
    app.cleanup().await;
}

#[tokio::test]
async fn preview_details_are_bounded_without_truncating_summary_counts() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut value = bundle();
    value["resources"]["dictionaries"][0]["items"]=Value::Array((0..150).map(|index|json!({"label":format!("选项{index}"),"value":format!("value-{index:03}"),"sort":index,"enabled":true})).collect());
    let result = preview(&app, token, value, "merge").await;
    assert_eq!(result["counts"]["dictionaryItems"]["create"], 150);
    assert_eq!(result["changes"].as_array().unwrap().len(), 100);
    assert_eq!(result["detailsTruncated"], true);
    app.cleanup().await;
}

#[tokio::test]
async fn conflicting_release_codes_cannot_be_overwritten_by_merge_preview() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (status, _) = app
        .send(
            "POST",
            "/api/v1/versions",
            json!({"name":"Other","code":"import-demo","apiOperationIds":["health.live"]}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let result = preview(&app, token, bundle(), "merge").await;
    assert_eq!(result["canApply"], false);
    assert_eq!(result["versionAction"], "conflict");
    app.cleanup().await;
}

#[tokio::test]
async fn small_imports_can_target_large_dictionaries_and_bind_unselected_item_writes() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (_, dictionary) = app
        .send(
            "POST",
            "/api/v1/dictionaries",
            json!({"name":"状态","key":"import-status"}),
            Some(token),
        )
        .await;
    let id = mongodb::bson::oid::ObjectId::parse_str(dictionary["data"]["id"].as_str().unwrap())
        .unwrap();
    let now = mongodb::bson::DateTime::now();
    let rows=(0..15000).map(|index|doc!{"dictionaryId":id,"label":"Extra","value":format!("extra-{index:05}"),"sort":index,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("dictionary_items")
        .insert_many(rows)
        .await
        .unwrap();
    let first = preview(&app, token, bundle(), "merge").await;
    assert_eq!(first["canApply"], true);
    assert_eq!(first["counts"]["dictionaryItems"]["create"], 1);
    let row = app
        .database
        .collection::<Document>("dictionary_items")
        .find_one(doc! {"dictionaryId":id,"value":"extra-00000"})
        .await
        .unwrap()
        .unwrap();
    let (status, _) = app
        .send(
            "PATCH",
            &format!(
                "/api/v1/dictionaries/{}/items/{}",
                id.to_hex(),
                row.get_object_id("_id").unwrap().to_hex()
            ),
            json!({"version":1,"label":"Changed extra"}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let second = preview(&app, token, bundle(), "merge").await;
    let docs = app.database.collection::<Document>("version_previews");
    let one=docs.find_one(doc!{"_id":mongodb::bson::oid::ObjectId::parse_str(first["id"].as_str().unwrap()).unwrap()}).await.unwrap().unwrap();
    let two=docs.find_one(doc!{"_id":mongodb::bson::oid::ObjectId::parse_str(second["id"].as_str().unwrap()).unwrap()}).await.unwrap().unwrap();
    assert_ne!(
        one.get_str("baselineHash").unwrap(),
        two.get_str("baselineHash").unwrap()
    );
    assert_eq!(
        app.database
            .collection::<Document>("dictionary_items")
            .count_documents(doc! {"dictionaryId":id,"deletedAt":null})
            .await
            .unwrap(),
        15000
    );
    app.cleanup().await;
}
