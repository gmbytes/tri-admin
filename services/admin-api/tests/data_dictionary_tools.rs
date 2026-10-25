mod support;
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
fn csv(resource: &str, text: &str) -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":resource,"format":"csv","contentBase64":STANDARD.encode(text)})
}
async fn preview(app: &TestApp, access: &str, body: Value) -> Value {
    let result = app
        .send("POST", "/api/v1/data-tools/imports", body, Some(access))
        .await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    result.1["data"].clone()
}
async fn dictionary(app: &TestApp, access: &str, key: &str) -> Value {
    let result = app
        .send(
            "POST",
            "/api/v1/dictionaries",
            json!({"name":key,"key":key}),
            Some(access),
        )
        .await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    result.1["data"].clone()
}
#[tokio::test]
async fn dictionaries_import_typed_flags_once_and_export_both_formats() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let job=preview(&app,access,csv("dictionaries","name,key,description,enabled\n中文字典,import.dictionary,说明,false\n无效字典,import.invalid,,maybe\n")).await;
    assert_eq!(job["rows"][0]["values"]["enabled"], false);
    let expires = job["expiresAt"]
        .as_str()
        .expect("public expiry must be RFC3339 text");
    assert!(chrono::DateTime::parse_from_rfc3339(expires).is_ok());
    assert_eq!(job["rows"][1]["status"], "invalid");
    let path = format!(
        "/api/v1/data-tools/imports/{}/apply",
        job["id"].as_str().unwrap()
    );
    let (a, b) = tokio::join!(
        app.send("POST", &path, json!({"rows":[0,1]}), Some(access)),
        app.send("POST", &path, json!({"rows":[0]}), Some(access))
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    assert_eq!(
        app.database
            .collection::<Document>("dictionaries")
            .count_documents(doc! {"key":"import.dictionary"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dictionaries.create"})
            .await
            .unwrap(),
        1
    );
    for format in ["csv", "xlsx"] {
        let result = app
            .send(
                "POST",
                "/api/v1/data-tools/export",
                json!({"resource":"dictionaries","format":format,"search":"import.dictionary"}),
                Some(access),
            )
            .await;
        assert_eq!(result.0, StatusCode::OK, "{}", result.1);
        assert_eq!(result.1["data"]["rowCount"], 1);
        let bytes = STANDARD
            .decode(result.1["data"]["contentBase64"].as_str().unwrap())
            .unwrap();
        let table = admin_api::modules::data_tools::formats::decode(
            if format == "csv" {
                admin_api::modules::data_tools::formats::Format::Csv
            } else {
                admin_api::modules::data_tools::formats::Format::Xlsx
            },
            &bytes,
        )
        .unwrap();
        assert_eq!(table.rows[0][0].text(), "中文字典");
        assert_eq!(table.rows[0][3].text(), "false");
    }
    app.cleanup().await;
}
#[tokio::test]
async fn items_bind_parent_identity_detect_duplicates_and_export_only_active_parents() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let parent = dictionary(&app, access, "import.parent").await;
    dictionary(&app, access, "import.other").await;
    let job=preview(&app,access,csv("dictionaryItems","dictionaryKey,label,value,sort,enabled\nimport.parent,第一项,001,-3,false\nimport.other,另一父级,001,7,true\nmissing.parent,不存在,x,0,true\nimport.parent,重复1,dup,0,true\nimport.parent,重复2,dup,1,true\n")).await;
    assert_eq!(job["rows"][0]["status"], "ready");
    assert_eq!(job["rows"][2]["error"], "PARENT_NOT_FOUND");
    assert_eq!(job["rows"][3]["status"], "duplicate");
    let id = job["id"].as_str().unwrap();
    let result = app
        .send(
            "POST",
            &format!("/api/v1/data-tools/imports/{id}/apply"),
            json!({"rows":[0,1,2,3,4]}),
            Some(access),
        )
        .await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert_eq!(result.1["data"]["rows"][0]["status"], "applied");
    assert_eq!(result.1["data"]["rows"][1]["status"], "applied");
    let exported = app
        .send(
            "POST",
            "/api/v1/data-tools/export",
            json!({"resource":"dictionaryItems","format":"xlsx","search":"import.parent"}),
            Some(access),
        )
        .await;
    assert_eq!(exported.0, StatusCode::OK, "{}", exported.1);
    assert_eq!(exported.1["data"]["rowCount"], 1);
    let bytes = STANDARD
        .decode(exported.1["data"]["contentBase64"].as_str().unwrap())
        .unwrap();
    let table = admin_api::modules::data_tools::formats::decode(
        admin_api::modules::data_tools::formats::Format::Xlsx,
        &bytes,
    )
    .unwrap();
    assert_eq!(table.rows[0][2].text(), "001");
    assert_eq!(table.rows[0][3].text(), "-3");
    assert_eq!(table.rows[0][4].text(), "false");
    // A parent rename/replacement cannot redirect an already-previewed row.
    let bound = preview(
        &app,
        access,
        csv(
            "dictionaryItems",
            "dictionaryKey,label,value\nimport.parent,待创建,new\n",
        ),
    )
    .await;
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/dictionaries/{}", parent["id"].as_str().unwrap()),
            json!({"version":1,"key":"import.renamed"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    dictionary(&app, access, "import.parent").await;
    let result = app
        .send(
            "POST",
            &format!(
                "/api/v1/data-tools/imports/{}/apply",
                bound["id"].as_str().unwrap()
            ),
            json!({"rows":[0]}),
            Some(access),
        )
        .await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert_eq!(result.1["data"]["rows"][0]["error"], "PARENT_CHANGED");
    assert_eq!(
        app.database
            .collection::<Document>("dictionary_items")
            .count_documents(doc! {"value":"new"})
            .await
            .unwrap(),
        0
    );
    app.database
        .collection::<Document>("dictionaries")
        .update_one(
            doc! {"_id":ObjectId::parse_str(parent["id"].as_str().unwrap()).unwrap()},
            doc! {"$set":{"deletedAt":mongodb::bson::DateTime::now()}},
        )
        .await
        .unwrap();
    let exported = app
        .send(
            "POST",
            "/api/v1/data-tools/export",
            json!({"resource":"dictionaryItems","format":"csv","search":"import.renamed"}),
            Some(access),
        )
        .await;
    assert_eq!(exported.1["data"]["rowCount"], 0);
    app.cleanup().await;
}
#[tokio::test]
async fn imported_item_creation_and_parent_deletion_preserve_the_parent_guard() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let parent = dictionary(&app, access, "race.parent").await;
    let job = preview(
        &app,
        access,
        csv(
            "dictionaryItems",
            "dictionaryKey,label,value\nrace.parent,竞态,one\n",
        ),
    )
    .await;
    let apply = format!(
        "/api/v1/data-tools/imports/{}/apply",
        job["id"].as_str().unwrap()
    );
    let delete = format!(
        "/api/v1/dictionaries/{}?version=1",
        parent["id"].as_str().unwrap()
    );
    let (created, deleted) = tokio::join!(
        app.send("POST", &apply, json!({"rows":[0]}), Some(access)),
        app.send("DELETE", &delete, Value::Null, Some(access))
    );
    assert_eq!(created.0, StatusCode::OK, "{}", created.1);
    let count=app.database.collection::<Document>("dictionary_items").count_documents(doc!{"dictionaryId":ObjectId::parse_str(parent["id"].as_str().unwrap()).unwrap(),"deletedAt":null}).await.unwrap();
    if deleted.0 == StatusCode::NO_CONTENT {
        assert_eq!(count, 0);
        assert_eq!(created.1["data"]["rows"][0]["status"], "conflict");
    } else {
        assert_eq!(deleted.0, StatusCode::CONFLICT);
        assert_eq!(count, 1);
        assert_eq!(created.1["data"]["rows"][0]["status"], "applied");
    }
    app.cleanup().await;
}
#[tokio::test]
async fn dictionary_only_operators_use_resource_permissions_and_see_their_own_history() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let role=app.send("POST","/api/v1/roles",json!({"name":"dictionary-importer","slug":"dictionary-importer","permissions":["exports.read","exports.write","dictionaries.read","dictionaries.write"],"dataScope":"self"}),Some(access)).await.1["data"].clone();
    app.send("POST","/api/v1/users",json!({"username":"dictionary-importer","displayName":"Importer","password":PASSWORD,"roleIds":[role["id"]]}),Some(access)).await;
    let auth = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"dictionary-importer","password":PASSWORD}),
            None,
        )
        .await
        .1;
    let token = auth["data"]["accessToken"].as_str().unwrap();
    let job = preview(
        &app,
        token,
        csv("dictionaries", "name,key\n受控字典,only.dictionary\n"),
    )
    .await;
    let history = app
        .send(
            "GET",
            "/api/v1/data-tools/imports",
            Value::Null,
            Some(token),
        )
        .await
        .1;
    assert_eq!(history["data"][0]["id"], job["id"]);
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/data-tools/imports",
            csv(
                "parameters",
                "name,key,valueType,value\nWrong,wrong,string,x\n"
            ),
            Some(token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.send(
        "PATCH",
        &format!("/api/v1/roles/{}", role["id"].as_str().unwrap()),
        json!({"version":1,"permissions":["exports.read","exports.write","dictionaries.read"]}),
        Some(access),
    )
    .await;
    assert_eq!(
        app.send(
            "POST",
            &format!(
                "/api/v1/data-tools/imports/{}/apply",
                job["id"].as_str().unwrap()
            ),
            json!({"rows":[0]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn own_schema_upgrade_preserves_parameter_receipts_and_custom_navigation() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let job = preview(
        &app,
        access,
        csv(
            "parameters",
            "name,key,valueType,value\nRetained,upgrade.retained,string,value\n",
        ),
    )
    .await;
    let id = ObjectId::parse_str(job["id"].as_str().unwrap()).unwrap();
    app.database
        .collection::<Document>("data_imports")
        .update_one(doc! {"_id":id}, doc! {"$unset":{"rows.0.parentId":""}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"data-tools"}, doc! {"$set":{"version":1}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("menus")
        .update_one(
            doc! {"viewKey":"exports"},
            doc! {"$set":{"label":"自定义数据工具","version":9i64}},
        )
        .await
        .unwrap();
    assert!(
        !admin_api::storage::schema::is_initialized(&app.database)
            .await
            .unwrap()
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert!(
        admin_api::storage::schema::is_initialized(&app.database)
            .await
            .unwrap()
    );
    let raw = app
        .database
        .collection::<Document>("data_imports")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert!(
        raw.get_array("rows").unwrap()[0]
            .as_document()
            .unwrap()
            .get("parentId")
            .unwrap()
            .as_null()
            .is_some()
    );
    assert_eq!(
        app.database
            .collection::<Document>("menus")
            .find_one(doc! {"viewKey":"exports"})
            .await
            .unwrap()
            .unwrap()
            .get_str("label")
            .unwrap(),
        "自定义数据工具"
    );
    let apply = app
        .send(
            "POST",
            &format!("/api/v1/data-tools/imports/{id}/apply"),
            json!({"rows":[0]}),
            Some(access),
        )
        .await;
    assert_eq!(apply.0, StatusCode::OK, "{}", apply.1);
    assert_eq!(apply.1["data"]["rows"][0]["status"], "applied");
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/imports/{id}/apply"),
            json!({"rows":[0]}),
            Some(access)
        )
        .await
        .1["data"]["rows"][0]["targetId"],
        apply.1["data"]["rows"][0]["targetId"]
    );
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"data-tools"}, doc! {"$set":{"version":999}})
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
async fn a_large_preview_applies_one_hundred_high_offset_rows_with_bounded_row_reads() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    async fn send(app: &TestApp, path: &str, body: Value, access: &str) -> (StatusCode, Value) {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {access}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 12 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let mut text = String::from("name,key,valueType,value\n");
    let value = "x".repeat(10 * 1024);
    for i in 0..200 {
        text.push_str(&format!("Large,large.preview.{i:03},string,{value}\n"));
    }
    let job = send(
        &app,
        "/api/v1/data-tools/imports",
        csv("parameters", &text),
        access,
    )
    .await;
    assert_eq!(
        job.0,
        StatusCode::CREATED,
        "{}",
        job.1.get("error").unwrap_or(&Value::Null)
    );
    let id = job.1["data"]["id"].as_str().unwrap();
    let start = std::time::Instant::now();
    let applied = send(
        &app,
        &format!("/api/v1/data-tools/imports/{id}/apply"),
        json!({"rows":(100..200).collect::<Vec<_>>()}),
        access,
    )
    .await;
    assert_eq!(
        applied.0,
        StatusCode::OK,
        "{}",
        applied.1.get("error").unwrap_or(&Value::Null)
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(applied.1["data"]["rows"][99]["status"], "ready");
    assert_eq!(applied.1["data"]["rows"][100]["status"], "applied");
    assert_eq!(applied.1["data"]["rows"][199]["status"], "applied");
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        100
    );
    println!(
        "large preview 200 x 10KiB, applied100 elapsedMs={}",
        start.elapsed().as_millis()
    );
    app.cleanup().await;
}

#[tokio::test]
async fn blank_optional_csv_flags_and_sort_use_the_same_defaults_as_empty_xlsx_cells() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let job = preview(
        &app,
        access,
        csv(
            "dictionaries",
            "name,key,enabled\nDefault,defaults.dictionary,\n",
        ),
    )
    .await;
    assert_eq!(job["rows"][0]["status"], "ready");
    assert_eq!(job["rows"][0]["values"]["enabled"], true);
    dictionary(&app, access, "defaults.parent").await;
    let job = preview(
        &app,
        access,
        csv(
            "dictionaryItems",
            "dictionaryKey,label,value,sort,enabled\ndefaults.parent,Default,value,,\n",
        ),
    )
    .await;
    assert_eq!(job["rows"][0]["status"], "ready");
    assert_eq!(job["rows"][0]["values"]["sort"], 0);
    assert_eq!(job["rows"][0]["values"]["enabled"], true);
    app.cleanup().await;
}
