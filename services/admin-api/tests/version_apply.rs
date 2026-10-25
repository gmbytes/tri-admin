#[path = "support/config.rs"]
mod config;
mod support;
use axum::http::StatusCode;
use config::bundle;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

async fn preview(app: &TestApp, token: &str, bundle: Value, strategy: &str) -> Value {
    let (status, result) = app
        .send(
            "POST",
            "/api/v1/versions/import-previews",
            json!({"bundle":bundle,"strategy":strategy}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    result["data"].clone()
}
async fn apply(app: &TestApp, token: &str, preview: &Value) -> (StatusCode, Value) {
    app.send(
        "POST",
        &format!(
            "/api/v1/versions/import-previews/{}/apply",
            preview["id"].as_str().unwrap()
        ),
        json!({"checksum":preview["checksum"]}),
        Some(token),
    )
    .await
}
async fn identity(app: &TestApp, owner: &str, name: &str, codes: Value) -> (String, ObjectId) {
    let (status, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":name,"slug":name,"permissions":codes,"dataScope":"all"}),
            Some(owner),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let (status,user)=app.send("POST","/api/v1/users",json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    (
        login["data"]["accessToken"].as_str().unwrap().into(),
        ObjectId::parse_str(role["data"]["id"].as_str().unwrap()).unwrap(),
    )
}
async fn count(app: &TestApp, collection: &str) -> u64 {
    app.database
        .collection::<Document>(collection)
        .count_documents(doc! {"deletedAt":null})
        .await
        .unwrap()
}

#[tokio::test]
async fn apply_commits_every_resource_new_references_version_and_audit_together() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let baseline = count(&app, "menus").await;
    let plan = preview(&app, token, bundle(), "createOnly").await;
    let (status, result) = apply(&app, token, &plan).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["data"]["counts"], plan["counts"]);
    assert_eq!(count(&app, "menus").await, baseline + 2);
    assert_eq!(count(&app, "dictionaries").await, 1);
    assert_eq!(count(&app, "dictionary_items").await, 1);
    assert_eq!(count(&app, "parameters").await, 1);
    assert_eq!(count(&app, "versions").await, 1);
    let parent = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"path":"/import-parent","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    let child = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"path":"/import-child","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        child.get_object_id("parentId").unwrap(),
        parent.get_object_id("_id").unwrap()
    );
    let dictionary = app
        .database
        .collection::<Document>("dictionaries")
        .find_one(doc! {"key":"import-status","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    let item = app
        .database
        .collection::<Document>("dictionary_items")
        .find_one(doc! {"value":"enabled","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        item.get_object_id("dictionaryId").unwrap(),
        dictionary.get_object_id("_id").unwrap()
    );
    let parameter = app
        .database
        .collection::<Document>("parameters")
        .find_one(doc! {"key":"import.enabled","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parameter.get_str("valueJson").unwrap(), "false");
    let version_id = result["data"]["versionId"].as_str().unwrap();
    let (_, meta) = app
        .send(
            "GET",
            &format!("/api/v1/versions/{version_id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(meta["data"]["origin"], "import");
    assert_eq!(meta["data"]["checksum"], plan["checksum"]);
    let (status, file) = app
        .send(
            "GET",
            &format!("/api/v1/versions/{version_id}/download"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(file["format"], "tri-admin/config-bundle");
    assert!(file["resources"]["menus"][0].get("id").is_none());
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"versions.import.apply"})
            .await
            .unwrap(),
        1
    );
    let audit = app
        .database
        .collection::<Document>("audit_events")
        .find_one(doc! {"action":"versions.import.apply"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.get_str("checksum").unwrap(), plan["checksum"]);
    assert_eq!(
        audit.get_object_id("previewId").unwrap().to_hex(),
        plan["id"]
    );
    let counts = serde_json::to_value(audit.get_document("counts").unwrap()).unwrap();
    assert_eq!(counts, plan["counts"]);
    app.cleanup().await;
}

#[tokio::test]
async fn replay_returns_the_exact_stored_result_without_rewriting_and_survives_preview_expiry() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let plan = preview(&app, token, bundle(), "createOnly").await;
    let (status, first) = apply(&app, token, &plan).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let before = app
        .database
        .collection::<Document>("parameters")
        .find_one(doc! {"key":"import.enabled"})
        .await
        .unwrap()
        .unwrap();
    let id = ObjectId::parse_str(plan["id"].as_str().unwrap()).unwrap();
    let stored = app
        .database
        .collection::<Document>("version_previews")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.get_str("status").unwrap(), "applied");
    assert!(!stored.contains_key("cleanupAt") && !stored.contains_key("bundleJson"));
    app.database
        .collection::<Document>("version_previews")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let (status, second) = apply(&app, token, &plan).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(first, second);
    let after = app
        .database
        .collection::<Document>("parameters")
        .find_one(doc! {"key":"import.enabled"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.get_i64("version").unwrap(),
        after.get_i64("version").unwrap()
    );
    assert_eq!(count(&app, "versions").await, 1);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"versions.import.apply"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn wrong_actor_digest_expired_preview_and_create_only_conflicts_do_not_apply() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let plan = preview(&app, token, bundle(), "createOnly").await;
    let (other, _) = identity(
        &app,
        token,
        "other-importer",
        json!([
            "versions.write",
            "menus.read",
            "menus.write",
            "dictionaries.read",
            "dictionaries.write",
            "parameters.read",
            "parameters.write",
            "permissions.read"
        ]),
    )
    .await;
    assert_eq!(apply(&app, &other, &plan).await.0, StatusCode::NOT_FOUND);
    let mut wrong = plan.clone();
    wrong["checksum"] = json!("0".repeat(64));
    assert_eq!(apply(&app, token, &wrong).await.0, StatusCode::CONFLICT);
    app.database
        .collection::<Document>("version_previews")
        .update_one(
            doc! {"_id":ObjectId::parse_str(plan["id"].as_str().unwrap()).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert_eq!(apply(&app, token, &plan).await.0, StatusCode::CONFLICT);
    app.send(
        "POST",
        "/api/v1/parameters",
        json!({"name":"Other","key":"import.enabled","valueType":"boolean","value":true}),
        Some(token),
    )
    .await;
    let conflict = preview(&app, token, bundle(), "createOnly").await;
    assert_eq!(apply(&app, token, &conflict).await.0, StatusCode::CONFLICT);
    assert_eq!(count(&app, "versions").await, 0);
    assert_eq!(count(&app, "dictionaries").await, 0);
    app.cleanup().await;
}

#[tokio::test]
async fn target_version_changes_invalidate_pending_previews_even_when_values_are_equal() {
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
    let plan = preview(&app, token, bundle(), "merge").await;
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
    assert_eq!(apply(&app, token, &plan).await.0, StatusCode::CONFLICT);
    assert_eq!(count(&app, "versions").await, 0);
    assert_eq!(count(&app, "dictionaries").await, 0);
    app.cleanup().await;
}

#[tokio::test]
async fn apply_rechecks_write_permissions_after_preview_and_after_role_revocation() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let codes = json!([
        "versions.write",
        "menus.read",
        "dictionaries.read",
        "parameters.read",
        "permissions.read"
    ]);
    let (token, role) = identity(&app, owner, "limited-importer", codes.clone()).await;
    let plan = preview(&app, &token, bundle(), "merge").await;
    assert_eq!(apply(&app, &token, &plan).await.0, StatusCode::FORBIDDEN);
    let mut allowed = codes.as_array().unwrap().clone();
    allowed.extend([
        json!("menus.write"),
        json!("dictionaries.write"),
        json!("parameters.write"),
    ]);
    app.database.collection::<Document>("roles").update_one(doc!{"_id":role},doc!{"$set":{"permissions":allowed.iter().map(|value|value.as_str().unwrap().to_owned()).collect::<Vec<_>>()}}).await.unwrap();
    let plan = preview(&app, &token, bundle(), "merge").await;
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"_id":role},
            doc! {"$pull":{"permissions":"parameters.write"}},
        )
        .await
        .unwrap();
    assert_eq!(apply(&app, &token, &plan).await.0, StatusCode::FORBIDDEN);
    assert_eq!(count(&app, "versions").await, 0);
    app.cleanup().await;
}

#[tokio::test]
async fn merge_preserves_extra_items_updates_versions_and_validates_the_final_graph_as_one_batch() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (_, a) = app
        .send(
            "POST",
            "/api/v1/menus",
            json!({"label":"A","path":"/move-a"}),
            Some(token),
        )
        .await;
    let (_, b) = app
        .send(
            "POST",
            "/api/v1/menus",
            json!({"label":"B","path":"/move-b","parentId":a["data"]["id"]}),
            Some(token),
        )
        .await;
    let (_, dictionary) = app
        .send(
            "POST",
            "/api/v1/dictionaries",
            json!({"name":"状态","key":"import-status"}),
            Some(token),
        )
        .await;
    let id = dictionary["data"]["id"].as_str().unwrap();
    app.send(
        "POST",
        &format!("/api/v1/dictionaries/{id}/items"),
        json!({"label":"Old","value":"enabled","sort":1}),
        Some(token),
    )
    .await;
    app.send(
        "POST",
        &format!("/api/v1/dictionaries/{id}/items"),
        json!({"label":"Keep","value":"extra","sort":9}),
        Some(token),
    )
    .await;
    app.send(
        "POST",
        "/api/v1/parameters",
        json!({"name":"开关","key":"import.enabled","valueType":"boolean","value":true}),
        Some(token),
    )
    .await;
    let mut source = bundle();
    source["resources"]["menus"] = json!([{"label":"A","path":"/move-a","parentPath":"/move-b","viewKey":null,"icon":null,"permission":null,"sort":0,"enabled":true},{"label":"B","path":"/move-b","parentPath":null,"viewKey":null,"icon":null,"permission":null,"sort":0,"enabled":true}]);
    source["resources"]["dictionaries"][0]["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({"label":"New","value":"new","sort":2,"enabled":true}));
    let plan = preview(&app, token, source, "merge").await;
    let (status, result) = apply(&app, token, &plan).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let current = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"path":"/move-a"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.get_object_id("parentId").unwrap().to_hex(),
        b["data"]["id"]
    );
    let current = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"path":"/move-b"})
        .await
        .unwrap()
        .unwrap();
    assert!(current.get("parentId").unwrap().as_null().is_some());
    let dictionary_id = ObjectId::parse_str(id).unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("dictionary_items")
            .count_documents(doc! {"dictionaryId":dictionary_id,"deletedAt":null})
            .await
            .unwrap(),
        3
    );
    let extra = app
        .database
        .collection::<Document>("dictionary_items")
        .find_one(doc! {"dictionaryId":dictionary_id,"value":"extra"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(extra.get_str("label").unwrap(), "Keep");
    assert_eq!(extra.get_i64("version").unwrap(), 1);
    let changed = app
        .database
        .collection::<Document>("dictionary_items")
        .find_one(doc! {"dictionaryId":dictionary_id,"value":"enabled"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.get_i64("version").unwrap(), 2);
    let parameter = app
        .database
        .collection::<Document>("parameters")
        .find_one(doc! {"key":"import.enabled"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parameter.get_i64("version").unwrap(), 2);
    assert_eq!(parameter.get_str("valueJson").unwrap(), "false");
    app.cleanup().await;
}

#[tokio::test]
async fn payload_failure_after_configuration_writes_rolls_back_everything_and_keeps_preview_pending()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let before = count(&app, "menus").await;
    let plan = preview(&app, token, bundle(), "merge").await;
    app.database.run_command(doc!{"collMod":"version_packages","validator":{"$jsonSchema":{"required":["rejectFixture"]}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        apply(&app, token, &plan).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(count(&app, "menus").await, before);
    for collection in ["dictionaries", "dictionary_items", "parameters", "versions"] {
        assert_eq!(count(&app, collection).await, 0);
    }
    let preview = app
        .database
        .collection::<Document>("version_previews")
        .find_one(doc! {"_id":ObjectId::parse_str(plan["id"].as_str().unwrap()).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preview.get_str("status").unwrap(), "pending");
    assert!(preview.contains_key("bundleJson"));
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"versions.import.apply"})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_replay_is_exactly_once_and_another_old_preview_is_rejected() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let first = preview(&app, token, bundle(), "merge").await;
    let second = preview(&app, token, bundle(), "merge").await;
    let (one, two) = tokio::join!(apply(&app, token, &first), apply(&app, token, &first));
    assert_eq!(one.0, StatusCode::OK, "{one:?}");
    assert_eq!(two.0, StatusCode::OK, "{two:?}");
    assert_eq!(one.1, two.1);
    assert_eq!(apply(&app, token, &second).await.0, StatusCode::CONFLICT);
    assert_eq!(count(&app, "versions").await, 1);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"versions.import.apply"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn an_identical_active_release_is_reused_but_a_retired_or_different_release_is_not_overwritten()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let first = preview(&app, token, bundle(), "merge").await;
    let (status, result) = apply(&app, token, &first).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (_, parameters) = app
        .send(
            "GET",
            "/api/v1/parameters?search=import.enabled",
            Value::Null,
            Some(token),
        )
        .await;
    let id = parameters["data"][0]["id"].as_str().unwrap();
    app.send(
        "PATCH",
        &format!("/api/v1/parameters/{id}"),
        json!({"version":1,"value":true}),
        Some(token),
    )
    .await;
    let restored = preview(&app, token, bundle(), "merge").await;
    assert_eq!(restored["versionAction"], "reuse");
    let (status, reused) = apply(&app, token, &restored).await;
    assert_eq!(status, StatusCode::OK, "{reused}");
    assert_eq!(reused["data"]["versionId"], result["data"]["versionId"]);
    assert_eq!(reused["data"]["versionReused"], true);
    assert_eq!(count(&app, "versions").await, 1);
    let version = result["data"]["versionId"].as_str().unwrap();
    app.send(
        "DELETE",
        &format!("/api/v1/versions/{version}?version=1"),
        Value::Null,
        Some(token),
    )
    .await;
    let retired = preview(&app, token, bundle(), "merge").await;
    assert_eq!(apply(&app, token, &retired).await.0, StatusCode::CONFLICT);
    app.cleanup().await;
}

#[tokio::test]
async fn a_concurrent_normal_edit_and_import_cannot_silently_overwrite_each_other() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (_, parameter) = app
        .send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"开关","key":"import.enabled","valueType":"boolean","value":true}),
            Some(token),
        )
        .await;
    let plan = preview(&app, token, bundle(), "merge").await;
    let path = format!(
        "/api/v1/parameters/{}",
        parameter["data"]["id"].as_str().unwrap()
    );
    let (import, edit) = tokio::join!(
        apply(&app, token, &plan),
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"description":"Concurrent edit"}),
            Some(token)
        )
    );
    assert!(
        matches!(
            (import.0, edit.0),
            (StatusCode::OK, StatusCode::CONFLICT) | (StatusCode::CONFLICT, StatusCode::OK)
        ),
        "import={import:?}, edit={edit:?}"
    );
    let stored = app
        .database
        .collection::<Document>("parameters")
        .find_one(doc! {"key":"import.enabled","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.get_i64("version").unwrap(), 2);
    assert_eq!(
        stored.get_str("valueJson").unwrap(),
        if import.0 == StatusCode::OK {
            "false"
        } else {
            "true"
        }
    );
    assert_eq!(
        count(&app, "versions").await,
        u64::from(import.0 == StatusCode::OK)
    );
    app.cleanup().await;
}

#[tokio::test]
async fn unselected_dictionary_edits_and_revoked_reads_invalidate_application() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let (_, dictionary) = app
        .send(
            "POST",
            "/api/v1/dictionaries",
            json!({"name":"状态","key":"import-status"}),
            Some(owner),
        )
        .await;
    let dictionary_id = dictionary["data"]["id"].as_str().unwrap();
    let plan = preview(&app, owner, bundle(), "merge").await;
    let (status, _) = app
        .send(
            "POST",
            &format!("/api/v1/dictionaries/{dictionary_id}/items"),
            json!({"label":"Extra","value":"extra"}),
            Some(owner),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(apply(&app, owner, &plan).await.0, StatusCode::CONFLICT);
    assert_eq!(count(&app, "versions").await, 0);
    let codes = json!([
        "versions.write",
        "menus.read",
        "menus.write",
        "dictionaries.read",
        "dictionaries.write",
        "parameters.read",
        "parameters.write",
        "permissions.read"
    ]);
    let (token, role) = identity(&app, owner, "read-revoked", codes).await;
    let plan = preview(&app, &token, bundle(), "merge").await;
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"_id":role},
            doc! {"$pull":{"permissions":"parameters.read"}},
        )
        .await
        .unwrap();
    assert_eq!(apply(&app, &token, &plan).await.0, StatusCode::FORBIDDEN);
    assert_eq!(count(&app, "parameters").await, 0);
    app.cleanup().await;
}
