mod support;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::FutureExt;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{PASSWORD, TestApp};
use tower::ServiceExt;

async fn post(app: &TestApp, token: &str, path: &str, body: Value) -> Value {
    let (status, result) = app.send("POST", path, body, Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{path} {result}");
    result["data"].clone()
}
async fn source(app: &TestApp, token: &str) -> Value {
    let parent = post(
        app,
        token,
        "/api/v1/menus",
        json!({"label":"Parent","path":"/release-parent"}),
    )
    .await;
    let child=post(app,token,"/api/v1/menus",json!({"label":"Child","path":"/release-child","viewKey":"parameters","permission":"parameters.read","parentId":parent["id"]})).await;
    let dictionary = post(
        app,
        token,
        "/api/v1/dictionaries",
        json!({"name":"状态","key":"release-status"}),
    )
    .await;
    let item = post(
        app,
        token,
        &format!(
            "/api/v1/dictionaries/{}/items",
            dictionary["id"].as_str().unwrap()
        ),
        json!({"label":"启用","value":"enabled","sort":5}),
    )
    .await;
    let parameter = post(
        app,
        token,
        "/api/v1/parameters",
        json!({"name":"功能开关","key":"release.enabled","valueType":"boolean","value":false}),
    )
    .await;
    json!({"parent":parent,"child":child,"dictionary":dictionary,"item":item,"parameter":parameter})
}
fn capture_input(source: &Value, code: &str) -> Value {
    json!({"name":"配置发版","code":code,"description":"新格式配置快照","menuIds":[source["child"]["id"]],"dictionaryIds":[source["dictionary"]["id"]],"parameterIds":[source["parameter"]["id"]],"apiOperationIds":["parameters.get"]})
}
async fn download(
    app: &TestApp,
    token: &str,
    id: &str,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/versions/{id}/download"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 9 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

#[tokio::test]
async fn capture_is_a_portable_immutable_snapshot_with_ancestors_items_typed_values_and_api_dependencies()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let src = source(&app, token).await;
    let record = post(
        &app,
        token,
        "/api/v1/versions",
        capture_input(&src, "release-1"),
    )
    .await;
    assert_eq!(
        record["counts"],
        json!({"menus":2,"ancestors":1,"dictionaries":1,"dictionaryItems":1,"parameters":1,"apis":1})
    );
    assert!(record.get("packageJson").is_none());
    assert_eq!(record["version"], 1);
    assert_eq!(record["createdBy"]["name"], "管理员");
    let id = record["id"].as_str().unwrap();
    let stored = app
        .database
        .collection::<Document>("versions")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert!(!stored.contains_key("packageJson"));
    let (status, headers, bytes) = download(&app, token, id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/json");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("release-1")
    );
    let checksum = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(record["checksum"], checksum);
    assert_eq!(record["byteLength"], bytes.len());
    let bundle: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(bundle["format"], "tri-admin/config-bundle");
    assert_eq!(bundle["schemaVersion"], 1);
    let menus = bundle["resources"]["menus"].as_array().unwrap();
    let child = menus
        .iter()
        .find(|menu| menu["path"] == "/release-child")
        .unwrap();
    assert_eq!(child["parentPath"], "/release-parent");
    assert!(child.get("id").is_none());
    assert!(child.get("parentId").is_none());
    assert_eq!(
        bundle["resources"]["dictionaries"][0]["items"][0]["value"],
        "enabled"
    );
    assert_eq!(bundle["resources"]["parameters"][0]["value"], false);
    assert_eq!(bundle["requiredApis"][0]["operationId"], "parameters.get");
    assert_eq!(
        bundle["requiredApis"][0]["requiredPermissions"],
        json!(["parameters.read"])
    );
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(!text.contains(src["child"]["id"].as_str().unwrap()));
    assert!(!text.contains(auth["user"]["id"].as_str().unwrap()));
    app.send(
        "PATCH",
        &format!(
            "/api/v1/parameters/{}",
            src["parameter"]["id"].as_str().unwrap()
        ),
        json!({"version":1,"value":true}),
        Some(token),
    )
    .await;
    assert_eq!(download(&app, token, id).await.2, bytes);
    app.database
        .collection::<Document>("version_packages")
        .update_one(
            doc! {"_id":ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"packageJson":"{}"}},
        )
        .await
        .unwrap();
    assert_eq!(download(&app, token, id).await.0, StatusCode::CONFLICT);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/versions/{id}"),
            json!({"version":1,"name":"overwrite"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn snapshot_capture_and_download_cannot_bypass_resource_read_permissions() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let src = source(&app, token).await;
    let record = post(
        &app,
        token,
        "/api/v1/versions",
        capture_input(&src, "private-config"),
    )
    .await;
    let role=post(&app,token,"/api/v1/roles",json!({"name":"version-viewer","slug":"version-viewer","permissions":["versions.read","versions.write"],"dataScope":"all"})).await;
    post(&app,token,"/api/v1/users",json!({"username":"version-viewer","displayName":"Version viewer","password":PASSWORD,"roleIds":[role["id"]]})).await;
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"version-viewer","password":PASSWORD}),
            None,
        )
        .await;
    let access = login["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/versions",
            capture_input(&src, "escape"),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send("GET", "/api/v1/versions", Value::Null, Some(access))
            .await
            .0,
        StatusCode::OK
    );
    let id = record["id"].as_str().unwrap();
    let (_, meta) = app
        .send(
            "GET",
            &format!("/api/v1/versions/{id}"),
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(meta["data"]["checksum"], record["checksum"]);
    assert!(meta["data"].get("resources").is_none());
    assert_eq!(download(&app, access, id).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        app.send("GET", "/api/v1/versions", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn capture_rejects_empty_duplicate_missing_unregistered_or_malformed_selections() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let src = source(&app, token).await;
    let id = src["child"]["id"].clone();
    for body in [
        json!({"name":"x","code":"empty"}),
        json!({"name":"","code":"invalid","menuIds":[id]}),
        json!({"name":"x","code":"bad code","menuIds":[id]}),
        json!({"name":"x","code":"invalid","menuIds":[id,id]}),
        json!({"name":"x","code":"invalid","parameterIds":[ObjectId::new().to_hex()]}),
        json!({"name":"x","code":"invalid","apiOperationIds":["missing.operation"]}),
        json!({"name":"x","code":"invalid","menuIds":["not-id"]}),
        json!({"name":"x","code":"invalid","menuIds":[id],"versionData":"old-shape"}),
    ] {
        assert_eq!(
            app.send("POST", "/api/v1/versions", body, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let stale = ObjectId::new();
    let now = DateTime::now();
    app.database.collection::<Document>("menus").insert_one(doc!{"_id":stale,"label":"Unsupported","path":"/unsupported-view","viewKey":"unregistered-view","permission":null,"icon":null,"parentId":null,"sort":0,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/versions",
            json!({"name":"x","code":"stale","menuIds":[stale.to_hex()]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.cleanup().await;
}

#[tokio::test]
async fn immutable_version_codes_stay_unique_after_soft_delete_and_queries_are_bounded() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let src = source(&app, token).await;
    let record = post(
        &app,
        token,
        "/api/v1/versions",
        capture_input(&src, "a.one"),
    )
    .await;
    post(
        &app,
        token,
        "/api/v1/versions",
        capture_input(&src, "abone"),
    )
    .await;
    let (_, list) = app
        .send(
            "GET",
            "/api/v1/versions?search=a.&sort=code",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(list["meta"]["total"], 1);
    assert_eq!(list["data"][0]["code"], "a.one");
    let (_, one) = app
        .send(
            "GET",
            "/api/v1/versions?pageSize=1&sort=name",
            Value::Null,
            Some(token),
        )
        .await;
    let (_, two) = app
        .send(
            "GET",
            "/api/v1/versions?pageSize=1&page=2&sort=name",
            Value::Null,
            Some(token),
        )
        .await;
    assert_ne!(one["data"][0]["id"], two["data"][0]["id"]);
    for query in ["page=0", "pageSize=101", "sort=packageJson", "extra=x"] {
        assert_eq!(
            app.send(
                "GET",
                &format!("/api/v1/versions?{query}"),
                Value::Null,
                Some(token)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let id = record["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/versions/{id}?version=2"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/versions/{id}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(download(&app, token, id).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/versions",
            capture_input(&src, "A.ONE"),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_capture_of_the_same_code_has_one_winner() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let src = source(&app, token).await;
    let input = capture_input(&src, "race");
    let (one, two) = tokio::join!(
        app.send("POST", "/api/v1/versions", input.clone(), Some(token)),
        app.send("POST", "/api/v1/versions", input, Some(token))
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&status| status == StatusCode::CREATED)
            .count(),
        1
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&status| status == StatusCode::CONFLICT)
            .count(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn capture_reads_multiple_collections_from_one_consistent_snapshot() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let menu = post(
        &app,
        token,
        "/api/v1/menus",
        json!({"label":"revision-0","path":"/consistency"}),
    )
    .await;
    let parameter = post(
        &app,
        token,
        "/api/v1/parameters",
        json!({"name":"revision","key":"revision.number","valueType":"number","value":0}),
    )
    .await;
    let menu_id = ObjectId::parse_str(menu["id"].as_str().unwrap()).unwrap();
    let parameter_id = ObjectId::parse_str(parameter["id"].as_str().unwrap()).unwrap();
    let database = app.database.clone();
    let writer = async {
        for number in 1..=25 {
            let mut session = database.client().start_session().await.unwrap();
            session.start_transaction().and_run(database.clone(),|session,database|async move {
                database.collection::<Document>("menus").update_one(doc!{"_id":menu_id},doc!{"$set":{"label":format!("revision-{number}")},"$inc":{"version":1i64}}).session(&mut *session).await?;
                database.collection::<Document>("parameters").update_one(doc!{"_id":parameter_id},doc!{"$set":{"valueJson":number.to_string()},"$inc":{"version":1i64}}).session(session).await?;Ok(())
            }.boxed()).await.unwrap();
        }
    };
    let reader = async {
        for number in 0..10 {
            let record=post(&app,token,"/api/v1/versions",json!({"name":"Consistency","code":format!("consistent-{number}"),"menuIds":[menu["id"]],"parameterIds":[parameter["id"]]})).await;
            let (_, _, bytes) = download(&app, token, record["id"].as_str().unwrap()).await;
            let bundle: Value = serde_json::from_slice(&bytes).unwrap();
            let revision = bundle["resources"]["menus"][0]["label"]
                .as_str()
                .unwrap()
                .strip_prefix("revision-")
                .unwrap()
                .parse::<u64>()
                .unwrap();
            assert_eq!(bundle["resources"]["parameters"][0]["value"], revision);
        }
    };
    tokio::join!(writer, reader);
    app.cleanup().await;
}

#[tokio::test]
async fn oversized_capture_is_rejected_without_a_release_or_success_audit() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let dictionary = post(
        &app,
        token,
        "/api/v1/dictionaries",
        json!({"name":"Large","key":"large"}),
    )
    .await;
    let id = ObjectId::parse_str(dictionary["id"].as_str().unwrap()).unwrap();
    let now = DateTime::now();
    let items=(0..9500).map(|index|doc!{"dictionaryId":id,"label":"字".repeat(100),"value":format!("{}{:04}","值".repeat(196),index),"sort":index,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("dictionary_items")
        .insert_many(items)
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/versions",
            json!({"name":"Large","code":"too-large","dictionaryIds":[dictionary["id"]]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        app.database
            .collection::<Document>("versions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"versions.create"})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn version_schema_is_required_for_readiness() {
    let app = TestApp::new().await;
    let metadata = app.database.collection::<Document>("_schema_migrations");
    assert_eq!(
        metadata
            .count_documents(doc! {"_id":"versions","version":2})
            .await
            .unwrap(),
        1
    );
    metadata
        .update_one(doc! {"_id":"versions"}, doc! {"$set":{"version":999}})
        .await
        .unwrap();
    assert!(
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .is_err()
    );
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}

#[tokio::test]
async fn failed_payload_write_rolls_back_the_version_metadata_and_audit() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let src = source(&app, token).await;
    if !app
        .database
        .list_collection_names()
        .await
        .unwrap()
        .contains(&"version_packages".to_owned())
    {
        app.database
            .create_collection("version_packages")
            .await
            .unwrap();
    }
    app.database.run_command(doc!{"collMod":"version_packages","validator":{"$jsonSchema":{"required":["rejectFixture"]}},"validationLevel":"strict"}).await.unwrap();
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/versions",
            capture_input(&src, "rollback"),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(
        app.database
            .collection::<Document>("versions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("version_packages")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"versions.create"})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn version_menu_migration_is_explicit_idempotent_and_preserves_user_changes() {
    let app = TestApp::new().await;
    let menus = app.database.collection::<Document>("menus");
    menus.delete_many(doc! {"path":"/versions"}).await.unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"versions"}, doc! {"$set":{"version":1}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let menu = menus
        .find_one(doc! {"path":"/versions","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(menu.get_str("label").unwrap(), "版本管理");
    assert_eq!(menu.get_str("viewKey").unwrap(), "versions");
    assert_eq!(menu.get_str("permission").unwrap(), "versions.read");
    let id = menu.get_object_id("_id").unwrap();
    menus
        .update_one(doc! {"_id":id}, doc! {"$set":{"label":"自定义版本名称"}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        menus
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap()
            .get_str("label")
            .unwrap(),
        "自定义版本名称"
    );
    menus
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"deletedAt":mongodb::bson::DateTime::now()}},
        )
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        menus
            .count_documents(doc! {"path":"/versions","deletedAt":null})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn differently_cased_hex_strings_cannot_select_the_same_resource_twice() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let src = source(&app, token).await;
    for (field, resource) in [
        ("menuIds", "child"),
        ("dictionaryIds", "dictionary"),
        ("parameterIds", "parameter"),
    ] {
        let id = src[resource]["id"].as_str().unwrap();
        let mut body = json!({"name":"Mixed","code":format!("mixed-{resource}")});
        body[field] = json!([id, id.to_ascii_uppercase()]);
        let (status, result) = app
            .send("POST", "/api/v1/versions", body, Some(token))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field} {result}");
    }
    app.cleanup().await;
}
