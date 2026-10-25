mod support;
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

fn input(csv: &str) -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"csv","contentBase64":STANDARD.encode(csv),"mapping":{}})
}

#[tokio::test]
async fn preview_mapping_types_duplicates_batch_partial_failure_and_replay_are_durable() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let mut body = input(
        "名称,标识,类型,值\n布尔,import.boolean,boolean,true\n数字,import.number,number,12.5\n错误,import.bad,boolean,maybe\n重复1,import.dup,string,one\n重复2,import.dup,string,two\n",
    );
    body["mapping"] = json!({"name":"名称","key":"标识","valueType":"类型","value":"值"});
    let (status, preview) = app
        .send(
            "POST",
            "/api/v1/data-tools/imports",
            body.clone(),
            Some(access),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let id = preview["data"]["id"].as_str().unwrap();
    assert_eq!(preview["data"]["rows"][0]["status"], "ready");
    assert_eq!(preview["data"]["rows"][2]["status"], "invalid");
    assert_eq!(preview["data"]["rows"][3]["status"], "duplicate");
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/data-tools/imports",
            body.clone(),
            Some(access)
        )
        .await
        .1["data"]["id"],
        id
    );
    body["contentBase64"] =
        json!(STANDARD.encode("名称,标识,类型,值\nChanged,other.key,string,x\n"));
    assert_eq!(
        app.send("POST", "/api/v1/data-tools/imports", body, Some(access))
            .await
            .0,
        StatusCode::CONFLICT
    );
    app.send(
        "POST",
        "/api/v1/parameters",
        json!({"name":"竞态","key":"import.number","valueType":"number","value":99}),
        Some(access),
    )
    .await;
    let path = format!("/api/v1/data-tools/imports/{id}/apply");
    let apply = json!({"rows":[0,1,2,3,4]});
    let (left, right) = tokio::join!(
        app.send("POST", &path, apply.clone(), Some(access)),
        app.send("POST", &path, apply.clone(), Some(access))
    );
    assert_eq!(left.0, StatusCode::OK, "{}", left.1);
    assert_eq!(right.0, StatusCode::OK, "{}", right.1);
    let result = app
        .send(
            "GET",
            &format!("/api/v1/data-tools/imports/{id}"),
            Value::Null,
            Some(access),
        )
        .await
        .1;
    assert_eq!(result["data"]["rows"][0]["status"], "applied");
    assert_eq!(result["data"]["rows"][1]["status"], "conflict");
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {"key":"import.boolean"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"parameters.create"})
            .await
            .unwrap(),
        2
    );
    let record = app
        .database
        .collection::<Document>("parameters")
        .find_one(doc! {"key":"import.boolean"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.get_str("valueJson").unwrap(), "true");
    assert_eq!(
        app.send("POST", &path, apply, Some(access)).await.0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn exports_are_whitelisted_typed_and_scoped_before_limit() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let role=app.send("POST","/api/v1/roles",json!({"name":"exporter","slug":"exporter","permissions":["exports.read","exports.write","users.read"],"dataScope":"self"}),Some(access)).await.1["data"].clone();
    app.send("POST","/api/v1/users",json!({"username":"exporter","displayName":"=literal name","password":PASSWORD,"roleIds":[role["id"]]}),Some(access)).await;
    let tokens = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"exporter","password":PASSWORD}),
            None,
        )
        .await
        .1;
    let limited = tokens["data"]["accessToken"].as_str().unwrap();
    for format in ["csv", "xlsx"] {
        let (status,result)=app.send("POST","/api/v1/data-tools/export",json!({"resource":"users","format":format,"fields":["username","displayName"],"search":""}),Some(limited)).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["data"]["rowCount"], 1);
        let bytes = STANDARD
            .decode(result["data"]["contentBase64"].as_str().unwrap())
            .unwrap();
        let f = if format == "csv" {
            admin_api::modules::data_tools::formats::Format::Csv
        } else {
            admin_api::modules::data_tools::formats::Format::Xlsx
        };
        let table = admin_api::modules::data_tools::formats::decode(f, &bytes).unwrap();
        assert_eq!(table.headers, vec!["username", "displayName"]);
        assert_eq!(table.rows[0][0].text(), "exporter");
        assert_eq!(
            table.rows[0][1].text(),
            if format == "csv" {
                "'=literal name"
            } else {
                "=literal name"
            }
        );
    }
    for fields in [
        json!(["passwordHash"]),
        json!(["username", "username"]),
        json!(["$where"]),
    ] {
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/data-tools/export",
                json!({"resource":"users","format":"csv","fields":fields}),
                Some(limited)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/data-tools/export",
            json!({"resource":"parameters","format":"csv"}),
            Some(limited)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/data-tools/imports",
            input("name,key,valueType,value\nTest,test.key,string,test\n"),
            Some(limited)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn imports_are_owner_only_permission_checked_expiring_and_reject_unknown_mapping() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let body = input("name,key,valueType,value\nTest,test.key,string,test\n");
    let result = app
        .send(
            "POST",
            "/api/v1/data-tools/imports",
            body.clone(),
            Some(access),
        )
        .await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    let id = result.1["data"]["id"].as_str().unwrap();
    let mut bad = body;
    bad["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    bad["mapping"] = json!({"password":"value"});
    assert_eq!(
        app.send("POST", "/api/v1/data-tools/imports", bad, Some(access))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let role = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"owner"})
        .await
        .unwrap()
        .unwrap();
    app.send("POST","/api/v1/users",json!({"username":"second-owner","displayName":"Other","password":PASSWORD,"roleIds":[role.get_object_id("_id").unwrap().to_hex()]}),Some(access)).await;
    let token = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"second-owner","password":PASSWORD}),
            None,
        )
        .await
        .1;
    let other = token["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/data-tools/imports/{id}"),
            Value::Null,
            Some(other)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":mongodb::bson::oid::ObjectId::parse_str(id).unwrap()},
            doc! {"$set":{"expiresAt":mongodb::bson::DateTime::from_millis(0)}},
        )
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
        .0,
        StatusCode::CONFLICT
    );
    app.cleanup().await;
}

#[tokio::test]
async fn roles_scope_export_tracks_current_membership_and_rejects_over_limit_all_scope() {
    let app = TestApp::new().await;
    let owner = app.login().await;
    let access = owner["accessToken"].as_str().unwrap();
    let team = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":"team","slug":"team","permissions":[],"dataScope":"self"}),
            Some(access),
        )
        .await
        .1["data"]
        .clone();
    let role=app.send("POST","/api/v1/roles",json!({"name":"scoped","slug":"scoped","permissions":["exports.read","exports.write","users.read"],"dataScope":"roles","dataScopeRoleIds":[team["id"]]}),Some(access)).await;
    assert_eq!(role.0, StatusCode::CREATED, "{}", role.1);
    for (name, role_id) in [
        ("scoped-exporter", &role.1["data"]["id"]),
        ("team-member", &team["id"]),
    ] {
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/users",
                json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role_id]}),
                Some(access)
            )
            .await
            .0,
            StatusCode::CREATED
        );
    }
    let rows=(0..2001).map(|i|doc!{"username":format!("outside-{i:04}"),"usernameKey":format!("outside-{i:04}"),"displayName":"Outside","status":"active","version":1i64,"roleIds":[],"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<Document>("users")
        .insert_many(rows)
        .await
        .unwrap();
    let tokens = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"scoped-exporter","password":PASSWORD}),
            None,
        )
        .await
        .1;
    let token = tokens["data"]["accessToken"].as_str().unwrap();
    let export = json!({"resource":"users","format":"csv"});
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/data-tools/export",
            export.clone(),
            Some(token)
        )
        .await
        .1["data"]["rowCount"],
        2
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/data-tools/export",
            export.clone(),
            Some(access)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.database
        .collection::<Document>("users")
        .update_one(
            doc! {"username":"team-member"},
            doc! {"$set":{"roleIds":[]}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send("POST", "/api/v1/data-tools/export", export, Some(token))
            .await
            .1["data"]["rowCount"],
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn preview_is_not_a_write_authorization_and_invalid_numbers_remain_readable() {
    let app = TestApp::new().await;
    let owner = app.login().await;
    let access = owner["accessToken"].as_str().unwrap();
    let permissions = json!([
        "exports.read",
        "exports.write",
        "parameters.read",
        "parameters.write"
    ]);
    let role=app.send("POST","/api/v1/roles",json!({"name":"importer","slug":"importer","permissions":permissions,"dataScope":"self"}),Some(access)).await.1["data"].clone();
    app.send("POST","/api/v1/users",json!({"username":"importer","displayName":"Importer","password":PASSWORD,"roleIds":[role["id"]]}),Some(access)).await;
    let tokens = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"importer","password":PASSWORD}),
            None,
        )
        .await
        .1;
    let token = tokens["data"]["accessToken"].as_str().unwrap();
    let result=app.send("POST","/api/v1/data-tools/imports",input("name,key,valueType,value\nValid,permit.valid,boolean,false\nUnsafe,permit.unsafe,number,9007199254740993\n"),Some(token)).await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    assert_eq!(result.1["data"]["rows"][1]["status"], "invalid");
    assert_eq!(
        result.1["data"]["rows"][1]["values"]["value"],
        "9007199254740993"
    );
    let id = result.1["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{}", role["id"].as_str().unwrap()),
            json!({"version":1,"permissions":["exports.read","exports.write","parameters.read"]}),
            Some(access)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/imports/{id}/apply"),
            json!({"rows":[0]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {"key":"permit.valid"})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/data-tools/imports/{id}"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn all_parameter_value_types_import_export_and_row_batch_limits_are_enforced() {
    use admin_api::modules::data_tools::formats::{self, Cell, Format, Table};
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let table = Table {
        headers: ["name", "key", "valueType", "value"]
            .iter()
            .map(|v| v.to_string())
            .collect(),
        rows: vec![
            vec![
                Cell::Text("Text".into()),
                Cell::Text("typed.text".into()),
                Cell::Text("string".into()),
                Cell::Text("00123".into()),
            ],
            vec![
                Cell::Text("Number".into()),
                Cell::Text("typed.number".into()),
                Cell::Text("number".into()),
                Cell::Number(-2.5),
            ],
            vec![
                Cell::Text("Boolean".into()),
                Cell::Text("typed.boolean".into()),
                Cell::Text("boolean".into()),
                Cell::Boolean(false),
            ],
            vec![
                Cell::Text("JSON".into()),
                Cell::Text("typed.json".into()),
                Cell::Text("json".into()),
                Cell::Text("{\"enabled\":true,\"items\":[null,1]}".into()),
            ],
        ],
    };
    let body = json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"xlsx","contentBase64":STANDARD.encode(formats::encode(Format::Xlsx,&table).unwrap())});
    let result = app
        .send("POST", "/api/v1/data-tools/imports", body, Some(access))
        .await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    let path = format!(
        "/api/v1/data-tools/imports/{}/apply",
        result.1["data"]["id"].as_str().unwrap()
    );
    for rows in [
        json!([]),
        json!([0, 0]),
        json!([4]),
        json!((0..101).collect::<Vec<_>>()),
    ] {
        assert_eq!(
            app.send("POST", &path, json!({"rows":rows}), Some(access))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let result = app
        .send("POST", &path, json!({"rows":[0,1,2,3]}), Some(access))
        .await;
    assert_eq!(result.0, StatusCode::OK, "{}", result.1);
    assert!(
        result.1["data"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "applied")
    );
    let result = app
        .send(
            "POST",
            "/api/v1/data-tools/export",
            json!({"resource":"parameters","format":"xlsx","search":"typed."}),
            Some(access),
        )
        .await;
    let bytes = STANDARD
        .decode(result.1["data"]["contentBase64"].as_str().unwrap())
        .unwrap();
    let exported = formats::decode(Format::Xlsx, &bytes).unwrap();
    assert_eq!(exported.rows.len(), 4);
    assert_eq!(exported.rows[0][5], Cell::Boolean(false));
    assert_eq!(
        exported.rows[1][5],
        Cell::Text("{\"enabled\":true,\"items\":[null,1]}".into())
    );
    assert_eq!(exported.rows[2][5], Cell::Number(-2.5));
    assert_eq!(exported.rows[3][5], Cell::Text("00123".into()));
    app.cleanup().await;
}
