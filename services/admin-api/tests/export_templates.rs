mod support;
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
fn template(code: &str, resource: &str) -> Value {
    json!({"name":"命名模板","code":code,"resource":resource,"columns":[{"field":"name","header":"名称"},{"field":"key","header":"标识"},{"field":"valueType","header":"类型"},{"field":"value","header":"数据"}],"search":"","filters":[],"maxRows":2000,"allowImport":true,"enabled":true})
}
#[tokio::test]
async fn templates_have_versioned_crud_copy_and_real_typed_export_and_import() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    for (key, group, value) in [
        ("named.keep", "red", json!(true)),
        ("named.skip", "blue", json!(false)),
    ] {
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/parameters",
                json!({"name":"数据","key":key,"group":group,"valueType":"boolean","value":value}),
                Some(access)
            )
            .await
            .0,
            StatusCode::CREATED
        );
    }
    let mut body = template("named.params", "parameters");
    body["filters"] = json!([{"field":"group","op":"eq","value":"red"}]);
    let created = app
        .send("POST", "/api/v1/export-templates", body, Some(access))
        .await;
    assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
    let id = created.1["data"]["id"].as_str().unwrap();
    let output = app
        .send(
            "POST",
            &format!("/api/v1/export-templates/{id}/export"),
            json!({"version":1,"format":"xlsx"}),
            Some(access),
        )
        .await;
    assert_eq!(output.0, StatusCode::OK, "{}", output.1);
    assert_eq!(output.1["data"]["rowCount"], 1);
    let bytes = STANDARD
        .decode(output.1["data"]["contentBase64"].as_str().unwrap())
        .unwrap();
    let table = admin_api::modules::data_tools::formats::decode(
        admin_api::modules::data_tools::formats::Format::Xlsx,
        &bytes,
    )
    .unwrap();
    assert_eq!(table.headers, vec!["名称", "标识", "类型", "数据"]);
    assert_eq!(
        table.rows[0][3],
        admin_api::modules::data_tools::formats::Cell::Boolean(true)
    );
    let copied = app
        .send(
            "POST",
            &format!("/api/v1/export-templates/{id}/copy"),
            json!({"version":1,"name":"复制模板","code":"named.copy"}),
            Some(access),
        )
        .await;
    assert_eq!(copied.0, StatusCode::CREATED, "{}", copied.1);
    assert_ne!(copied.1["data"]["id"], id);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/export-templates/{id}"),
            json!({"version":1,"name":"已修改"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/export-templates/{id}"),
            json!({"version":1,"name":"过期修改"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/export-templates/{id}/export"),
            json!({"version":1,"format":"csv"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let input = json!({"version":2,"requestId":uuid::Uuid::new_v4().to_string(),"format":"csv","contentBase64":STANDARD.encode("名称,标识,类型,数据\n导入,named.imported,boolean,false\n"),"mapping":{"name":"名称","key":"标识","valueType":"类型","value":"数据"}});
    let preview = app
        .send(
            "POST",
            &format!("/api/v1/export-templates/{id}/import-previews"),
            input.clone(),
            Some(access),
        )
        .await;
    assert_eq!(preview.0, StatusCode::CREATED, "{}", preview.1);
    let job = preview.1["data"]["id"].as_str().unwrap();
    assert_eq!(preview.1["data"]["template"]["version"], 2);
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/imports/{job}/apply"),
            json!({"rows":[0]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/export-templates/{id}?version=2"),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/export-templates/{id}"),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/data-tools/imports/{job}"),
            Value::Null,
            Some(access)
        )
        .await
        .1["data"]["template"]["name"],
        "已修改"
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {"key":"named.imported","deletedAt":null})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn templates_reject_unsafe_fields_conditions_headers_and_missing_import_columns() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    for change in [
        json!({"columns":[{"field":"passwordHash","header":"秘密"}]}),
        json!({"columns":[{"field":"name","header":"=formula"}]}),
        json!({"filters":[{"field":"$where","op":"eq","value":"expression"}]}),
        json!({"filters":[{"field":"group","op":"eq","value":{"$ne":null}}]}),
        json!({"columns":[{"field":"name","header":"相同"},{"field":"key","header":"相同"}]}),
        json!({"maxRows":0}),
        json!({"sql":"arbitrary"}),
    ] {
        let mut body = template(
            &format!("invalid.{}", uuid::Uuid::new_v4().simple()),
            "parameters",
        );
        for (k, v) in change.as_object().unwrap() {
            body[k] = v.clone()
        }
        assert_eq!(
            app.send("POST", "/api/v1/export-templates", body, Some(access))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    app.cleanup().await;
}
#[tokio::test]
async fn user_template_export_always_intersects_current_scope_and_resource_permission() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let body = json!({"name":"用户资料","code":"named.users","resource":"users","columns":[{"field":"username","header":"账号"},{"field":"displayName","header":"名称"}],"filters":[],"allowImport":false});
    let created = app
        .send("POST", "/api/v1/export-templates", body, Some(access))
        .await;
    assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
    let id = created.1["data"]["id"].as_str().unwrap();
    let role=app.send("POST","/api/v1/roles",json!({"name":"template-viewer","slug":"template-viewer","permissions":["exports.read","exports.write","users.read"],"dataScope":"self"}),Some(access)).await.1["data"].clone();
    app.send("POST","/api/v1/users",json!({"username":"template-viewer","displayName":"Viewer","password":PASSWORD,"roleIds":[role["id"]]}),Some(access)).await;
    let auth = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"template-viewer","password":PASSWORD}),
            None,
        )
        .await
        .1;
    let token = auth["data"]["accessToken"].as_str().unwrap();
    let result = app
        .send(
            "POST",
            &format!("/api/v1/export-templates/{id}/export"),
            json!({"version":1,"format":"csv"}),
            Some(token),
        )
        .await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert_eq!(result.1["data"]["rowCount"], 1);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{}", role["id"].as_str().unwrap()),
            json!({"version":1,"permissions":["exports.read","exports.write"]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/export-templates/{id}/export"),
            json!({"version":1,"format":"csv"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_edits_code_reuse_and_request_lookup_preserve_template_identity() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let record = app
        .send(
            "POST",
            "/api/v1/export-templates",
            template("identity.template", "parameters"),
            Some(access),
        )
        .await
        .1["data"]
        .clone();
    let id = record["id"].as_str().unwrap();
    let path = format!("/api/v1/export-templates/{id}");
    let (a, b) = tokio::join!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"name":"One"}),
            Some(access)
        ),
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"name":"Two"}),
            Some(access)
        )
    );
    assert!(
        a.0 == StatusCode::OK && b.0 == StatusCode::CONFLICT
            || b.0 == StatusCode::OK && a.0 == StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":2,"code":"cannot.change"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let request = uuid::Uuid::new_v4().to_string();
    let body = json!({"version":2,"requestId":request,"format":"csv","contentBase64":STANDARD.encode("名称,标识,类型,数据\nIdentity,identity.import,string,value\n")});
    let preview = app
        .send(
            "POST",
            &format!("{path}/import-previews"),
            body,
            Some(access),
        )
        .await;
    assert_eq!(preview.0, StatusCode::CREATED, "{}", preview.1);
    let job = preview.1["data"]["id"].clone();
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
    let replacement = app
        .send(
            "POST",
            "/api/v1/export-templates",
            template("identity.template", "parameters"),
            Some(access),
        )
        .await;
    assert_eq!(replacement.0, StatusCode::CREATED);
    assert_ne!(replacement.1["data"]["id"], id);
    let found = app
        .send(
            "GET",
            &format!("/api/v1/data-tools/imports/by-request/{request}"),
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(found.0, StatusCode::OK);
    assert_eq!(found.1["data"]["id"], job);
    assert_eq!(found.1["data"]["template"]["id"], id);
    app.cleanup().await;
}

#[tokio::test]
async fn controlled_item_filters_and_global_sort_apply_before_export_limit() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    for (key, values) in [
        ("query.a", vec![("a", 4, true), ("b", 1, true)]),
        ("query.b", vec![("c", 3, true), ("d", 10, false)]),
    ] {
        let parent = app
            .send(
                "POST",
                "/api/v1/dictionaries",
                json!({"name":key,"key":key}),
                Some(access),
            )
            .await
            .1["data"]
            .clone();
        for (value, sort, enabled) in values {
            assert_eq!(
                app.send(
                    "POST",
                    &format!(
                        "/api/v1/dictionaries/{}/items",
                        parent["id"].as_str().unwrap()
                    ),
                    json!({"label":value,"value":value,"sort":sort,"enabled":enabled}),
                    Some(access)
                )
                .await
                .0,
                StatusCode::CREATED
            );
        }
    }
    let body = json!({"resource":"dictionaryItems","format":"xlsx","fields":["value","sort","enabled"],"search":"query.","filters":[{"field":"enabled","op":"eq","value":true},{"field":"sort","op":"gte","value":2}],"sort":{"field":"sort","direction":"desc"}});
    let result = app
        .send("POST", "/api/v1/data-tools/export", body, Some(access))
        .await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert_eq!(result.1["data"]["rowCount"], 2);
    let bytes = STANDARD
        .decode(result.1["data"]["contentBase64"].as_str().unwrap())
        .unwrap();
    let table = admin_api::modules::data_tools::formats::decode(
        admin_api::modules::data_tools::formats::Format::Xlsx,
        &bytes,
    )
    .unwrap();
    assert_eq!(table.rows[0][0].text(), "a");
    assert_eq!(table.rows[1][0].text(), "c");
    app.cleanup().await;
}

#[tokio::test]
async fn own_v3_upgrade_never_restores_redacted_payload_or_loses_receipts() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let preview=app.send("POST","/api/v1/data-tools/imports",json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"csv","contentBase64":STANDARD.encode("name,key,valueType,value\nOld,upgrade.template,string,private\n")}),Some(access)).await.1["data"].clone();
    let id = mongodb::bson::oid::ObjectId::parse_str(preview["id"].as_str().unwrap()).unwrap();
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"expiresAt":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    admin_api::modules::data_tools::retention::maintenance_once(&app.database)
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_imports")
        .update_one(doc! {"_id":id}, doc! {"$unset":{"template":""}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"data-tools"}, doc! {"$set":{"version":3}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let result = app
        .send(
            "GET",
            &format!("/api/v1/data-tools/imports/{id}"),
            Value::Null,
            Some(access),
        )
        .await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert_eq!(result.1["data"]["payloadState"], "redacted");
    assert!(result.1["data"]["template"].is_null());
    assert!(result.1["data"]["rows"][0]["values"].is_null());
    app.cleanup().await;
}

#[tokio::test]
async fn typed_date_integer_filters_and_explicit_limits_fail_without_silent_truncation() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    for key in ["limit.one", "limit.two"] {
        app.send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"Limit","key":key,"valueType":"string","value":"value"}),
            Some(access),
        )
        .await;
    }
    let mut body = template("limit.template", "parameters");
    body["search"] = json!("limit.");
    body["maxRows"] = json!(1);
    body["filters"] = json!([{"field":"version","op":"gte","value":1},{"field":"createdAt","op":"gte","value":"1970-01-01T00:00:00Z"}]);
    let created = app
        .send("POST", "/api/v1/export-templates", body, Some(access))
        .await;
    assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
    let id = created.1["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/export-templates/{id}/export"),
            json!({"version":1,"format":"csv"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for filters in [
        json!([{"field":"createdAt","op":"gte","value":"not a date"}]),
        json!([{"field":"version","op":"eq","value":1.5}]),
        json!([{"field":"key","op":"in","value":[{"$gt":""}]}]),
        json!([{"field":"key","op":"prefix","value":"bad\u{0}prefix"}]),
    ] {
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/data-tools/export",
                json!({"resource":"parameters","format":"csv","filters":filters}),
                Some(access)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/export-templates/{id}"),
            json!({"version":1,"enabled":false}),
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/export-templates/{id}/download"),
            json!({"version":2,"format":"csv"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    app.cleanup().await;
}
