mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

async fn create(app: &TestApp, token: &str, key: &str, kind: &str, value: Value) -> Value {
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"业务参数","key":key,"valueType":kind,"value":value}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}

#[tokio::test]
async fn parameters_round_trip_json_types_without_bson_or_internal_fields() {
    let app = TestApp::new().await;
    let login = app.login().await;
    let token = login["accessToken"].as_str().unwrap();
    for (index, (kind, value)) in [
        ("string", json!("")),
        ("number", json!(1.25)),
        ("boolean", json!(false)),
        ("json", Value::Null),
        ("json", json!({"$oid":"ordinary text","a.b":[1,true,null]})),
    ]
    .into_iter()
    .enumerate()
    {
        let record = create(&app, token, &format!("typed-{index}"), kind, value.clone()).await;
        assert_eq!(record["value"], value);
        assert_eq!(record["valueType"], kind);
        assert!(record.get("valueJson").is_none());
        assert!(record.get("_id").is_none());
        assert_eq!(record["group"], "general");
        assert!(record["createdAt"].as_str().unwrap().ends_with('Z'));
        let (_, result) = app
            .send(
                "GET",
                &format!("/api/v1/parameters/{}", record["id"].as_str().unwrap()),
                Value::Null,
                Some(token),
            )
            .await;
        assert_eq!(result["data"]["value"], value);
    }
    app.cleanup().await;
}

#[tokio::test]
async fn parameter_patch_preserves_omitted_values_and_accepts_explicit_json_null_only() {
    let app = TestApp::new().await;
    let login = app.login().await;
    let token = login["accessToken"].as_str().unwrap();
    let record = create(&app, token, "patch", "string", json!("original")).await;
    let path = format!("/api/v1/parameters/{}", record["id"].as_str().unwrap());
    let (status, updated) = app
        .send(
            "PATCH",
            &path,
            json!({"version":1,"name":"重命名","description":""}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["value"], "original");
    for body in [
        json!({"version":2,"value":null}),
        json!({"version":2,"valueType":"boolean"}),
        json!({"version":2,"group":null}),
    ] {
        assert_eq!(
            app.send("PATCH", &path, body, Some(token)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let (status, updated) = app
        .send(
            "PATCH",
            &path,
            json!({"version":2,"valueType":"json","value":null}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["version"], 3);
    assert!(updated["data"]["value"].is_null());
    assert_eq!(
        app.send("PATCH", &path, json!({"version":2,"value":{}}), Some(token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let persisted = app
        .database
        .collection::<Document>("parameters")
        .find_one(doc! {"key":"patch"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted.get_str("valueJson").unwrap(), "null");
    let audit = app
        .database
        .collection::<Document>("audit_events")
        .find_one(doc! {"action":"parameters.update"})
        .await
        .unwrap()
        .unwrap();
    assert!(!audit.contains_key("value") && !audit.contains_key("valueJson"));
    app.cleanup().await;
}

#[tokio::test]
async fn parameter_validation_rejects_type_mismatches_unknown_fields_and_resource_limits() {
    let app = TestApp::new().await;
    let login = app.login().await;
    let token = login["accessToken"].as_str().unwrap();
    let mut deep = json!(1);
    for _ in 0..18 {
        deep = json!([deep]);
    }
    for body in [
        json!({"name":"x","key":"bad key","valueType":"string","value":""}),
        json!({"name":"","key":"valid","valueType":"string","value":""}),
        json!({"name":"x","key":"valid","valueType":"string","value":null}),
        json!({"name":"x","key":"valid","valueType":"number","value":"1"}),
        json!({"name":"x","key":"valid","valueType":"number","value":9007199254740993u64}),
        json!({"name":"x","key":"valid","valueType":"boolean","value":1}),
        json!({"name":"x","key":"valid","valueType":"xml","value":{}}),
        json!({"name":"x","key":"valid","valueType":"string","value":"x".repeat(16385)}),
        json!({"name":"x","key":"valid","valueType":"json","value":deep}),
        json!({"name":"x","key":"valid","valueType":"json","value":{},"extra":1}),
        json!({"name":"x","key":"valid","valueType":"json"}),
    ] {
        assert_eq!(
            app.send("POST", "/api/v1/parameters", body, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn parameter_uniqueness_soft_delete_search_and_stable_pages_are_enforced() {
    let app = TestApp::new().await;
    let login = app.login().await;
    let token = login["accessToken"].as_str().unwrap();
    let first = create(&app, token, "a.one", "number", json!(10)).await;
    create(&app, token, "abone", "boolean", json!(true)).await;
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"duplicate","key":"a.one","valueType":"json","value":{}}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, result) = app
        .send(
            "GET",
            "/api/v1/parameters?search=a.&sort=key",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(result["meta"]["total"], 1);
    assert_eq!(result["data"][0]["key"], "a.one");
    let (_, one) = app
        .send(
            "GET",
            "/api/v1/parameters?pageSize=1&sort=name",
            Value::Null,
            Some(token),
        )
        .await;
    let (_, two) = app
        .send(
            "GET",
            "/api/v1/parameters?pageSize=1&page=2&sort=name",
            Value::Null,
            Some(token),
        )
        .await;
    assert_ne!(one["data"][0]["id"], two["data"][0]["id"]);
    for query in ["page=0", "pageSize=101", "sort=valueJson", "extra=x"] {
        assert_eq!(
            app.send(
                "GET",
                &format!("/api/v1/parameters?{query}"),
                Value::Null,
                Some(token)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let path = format!("/api/v1/parameters/{}", first["id"].as_str().unwrap());
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{path}?version=0"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{path}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(token)).await.0,
        StatusCode::NOT_FOUND
    );
    create(&app, token, "a.one", "string", json!("reused")).await;
    app.cleanup().await;
}

#[tokio::test]
async fn parameters_require_permissions_and_permission_removal_is_immediate() {
    let app = TestApp::new().await;
    let login = app.login().await;
    let token = login["accessToken"].as_str().unwrap();
    let record = create(&app, token, "readable", "string", json!("hello")).await;
    let (_,role) = app.send("POST","/api/v1/roles",json!({"name":"参数查看","slug":"parameter-reader","permissions":["parameters.read"],"dataScope":"self"}),Some(token)).await;
    assert!(role["data"]["id"].is_string(), "{role}");
    let (status,_) = app.send("POST","/api/v1/users",json!({"username":"param-reader","displayName":"参数查看","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"param-reader","password":PASSWORD}),
            None,
        )
        .await;
    let reader = login["data"]["accessToken"].as_str().unwrap();
    let path = format!("/api/v1/parameters/{}", record["id"].as_str().unwrap());
    assert_eq!(
        app.send("GET", "/api/v1/parameters", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(reader)).await.0,
        StatusCode::OK
    );
    for (method, url) in [
        ("POST", "/api/v1/parameters".to_string()),
        ("PATCH", path.clone()),
        ("DELETE", format!("{path}?version=1")),
    ] {
        assert_eq!(
            app.send(method, &url, json!({}), Some(reader)).await.0,
            StatusCode::FORBIDDEN
        );
    }
    let (status, _) = app
        .send(
            "PATCH",
            &format!("/api/v1/roles/{}", role["data"]["id"].as_str().unwrap()),
            json!({"version":1,"permissions":[]}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(reader)).await.0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn parameter_concurrent_creates_and_updates_have_one_winner() {
    let app = TestApp::new().await;
    let login = app.login().await;
    let token = login["accessToken"].as_str().unwrap();
    let body = json!({"name":"race","key":"race","valueType":"number","value":1});
    let (one, two) = tokio::join!(
        app.send("POST", "/api/v1/parameters", body.clone(), Some(token)),
        app.send("POST", "/api/v1/parameters", body, Some(token))
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&s| s == StatusCode::CREATED)
            .count(),
        1
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    let record = if one.0 == StatusCode::CREATED {
        one.1
    } else {
        two.1
    };
    let path = format!(
        "/api/v1/parameters/{}",
        record["data"]["id"].as_str().unwrap()
    );
    let (one, two) = tokio::join!(
        app.send("PATCH", &path, json!({"version":1,"value":2}), Some(token)),
        app.send("PATCH", &path, json!({"version":1,"value":3}), Some(token))
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&s| s == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [one.0, two.0]
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn parameter_schema_is_required_and_unknown_versions_are_rejected() {
    let app = TestApp::new().await;
    let metadata = app.database.collection::<Document>("_schema_migrations");
    assert_eq!(
        metadata
            .count_documents(doc! {"_id":"parameters","version":1})
            .await
            .unwrap(),
        1
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    metadata
        .update_one(doc! {"_id":"parameters"}, doc! {"$set":{"version":999}})
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
async fn json_parameters_reject_nested_numbers_that_the_desktop_cannot_round_trip() {
    let app = TestApp::new().await;
    let login = app.login().await;
    let token = login["accessToken"].as_str().unwrap();
    let (status,body) = app.send("POST","/api/v1/parameters",json!({"name":"precision","key":"precision","valueType":"json","value":{"limits":[9007199254740993u64]}}),Some(token)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    app.cleanup().await;
}

#[tokio::test]
async fn parameter_default_and_prefix_queries_use_bounded_index_scans() {
    let app = TestApp::new().await;
    let now = mongodb::bson::DateTime::now();
    let records = (0..2000)
        .map(|index| {
            doc! {
                "name":"参数","key":format!("query{index:04}"),"group":"general","description":"",
                "valueType":"number","valueJson":index.to_string(),"version":1i64,
                "createdAt":now,"updatedAt":now,"deletedAt":null
            }
        })
        .collect::<Vec<_>>();
    app.database
        .collection::<Document>("parameters")
        .insert_many(records)
        .await
        .unwrap();
    for (filter, sort) in [
        (doc! {"deletedAt":null}, doc! {"createdAt":-1,"_id":-1}),
        (
            doc! {"deletedAt":null,"key":{"$regex":"^query00"}},
            doc! {"key":1,"_id":1},
        ),
    ] {
        let explain = app.database.run_command(doc!{"explain":{"find":"parameters","filter":filter,"sort":sort,"limit":20},"verbosity":"executionStats"}).await.unwrap();
        let statistics = explain.get_document("executionStats").unwrap();
        let number = |key| {
            statistics
                .get_i32(key)
                .map(i64::from)
                .or_else(|_| statistics.get_i64(key))
                .unwrap()
        };
        assert_eq!(number("nReturned"), 20);
        assert!(number("totalDocsExamined") <= 20, "{statistics:?}");
        assert!(number("totalKeysExamined") <= 25, "{statistics:?}");
        println!(
            "parameters: returned={}, docs={}, keys={} of 2000",
            number("nReturned"),
            number("totalDocsExamined"),
            number("totalKeysExamined")
        );
    }
    app.cleanup().await;
}
