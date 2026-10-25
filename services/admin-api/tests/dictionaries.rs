mod support;

use axum::http::StatusCode;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

async fn dictionary(app: &TestApp, token: &str, key: &str) -> Value {
    let (status, body) = app
        .send(
            "POST",
            "/api/v1/dictionaries",
            json!({"name":"地区","key":key,"description":"","enabled":true}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}
async fn item(app: &TestApp, token: &str, id: &str, value: &str, sort: i32) -> Value {
    let (status, body) = app
        .send(
            "POST",
            &format!("/api/v1/dictionaries/{id}/items"),
            json!({"label":value,"value":value,"sort":sort,"enabled":true}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["data"].clone()
}

#[tokio::test]
async fn dictionaries_and_items_have_versioned_updates_soft_deletes_and_parent_guards() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let token = tokens["accessToken"].as_str().unwrap();
    let record = dictionary(&app, token, "region").await;
    let id = record["id"].as_str().unwrap();
    assert!(record["createdAt"].as_str().unwrap().ends_with('Z'));
    assert!(record.get("_id").is_none());
    let path = format!("/api/v1/dictionaries/{id}");
    let child = item(&app, token, id, "cn", 10).await;
    let child_path = format!("{path}/items/{}", child["id"].as_str().unwrap());
    assert_eq!(child["dictionaryId"], id);
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{path}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, updated) = app
        .send(
            "PATCH",
            &path,
            json!({"version":1,"name":"国家地区","enabled":false}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["version"], 2);
    assert_eq!(updated["data"]["key"], "region");
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"name":"旧版本"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, updated) = app
        .send(
            "PATCH",
            &child_path,
            json!({"version":1,"label":"中国","enabled":false}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["value"], "cn");
    assert_eq!(updated["data"]["version"], 2);
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{child_path}?version=1"),
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
            &format!("{child_path}?version=2"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    item(&app, token, id, "cn", 0).await;
    let (_, list) = app
        .send("GET", &format!("{path}/items"), Value::Null, Some(token))
        .await;
    assert_eq!(list["meta"]["total"], 1);
    let active_child = list["data"][0]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{path}/items/{active_child}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{path}?version=2"),
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
    assert_eq!(
        app.send("GET", &format!("{path}/items"), Value::Null, Some(token))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    dictionary(&app, token, "region").await;
    assert!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":{"$regex":"^dictionaries\\."}})
            .await
            .unwrap()
            >= 8
    );
    app.cleanup().await;
}

#[tokio::test]
async fn dictionary_queries_are_literal_paginated_stable_and_whitelisted() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let token = tokens["accessToken"].as_str().unwrap();
    let first = dictionary(&app, token, "a.one").await;
    dictionary(&app, token, "abone").await;
    let (_, result) = app
        .send(
            "GET",
            "/api/v1/dictionaries?search=a.&sort=key",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(result["meta"]["total"], 1);
    assert_eq!(result["data"][0]["key"], "a.one");
    let id = first["id"].as_str().unwrap();
    item(&app, token, id, "a.one", 5).await;
    item(&app, token, id, "abone", 5).await;
    let path = format!("/api/v1/dictionaries/{id}/items");
    let (_, page1) = app
        .send(
            "GET",
            &format!("{path}?pageSize=1&sort=sort"),
            Value::Null,
            Some(token),
        )
        .await;
    let (_, page2) = app
        .send(
            "GET",
            &format!("{path}?pageSize=1&page=2&sort=sort"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(page1["meta"]["total"], 2);
    assert_ne!(page1["data"][0]["id"], page2["data"][0]["id"]);
    let (_, result) = app
        .send(
            "GET",
            &format!("{path}?search=a."),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(result["meta"]["total"], 1);
    let (_, empty) = app
        .send("GET", &format!("{path}?page=8"), Value::Null, Some(token))
        .await;
    assert_eq!(empty["data"], json!([]));
    for query in ["page=0", "pageSize=101", "sort=deletedAt", "unknown=x"] {
        assert_eq!(
            app.send("GET", &format!("{path}?{query}"), Value::Null, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{query}"
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn dictionary_validation_uniqueness_and_cross_parent_item_access_are_enforced() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let token = tokens["accessToken"].as_str().unwrap();
    for body in [
        json!({"name":"","key":"valid"}),
        json!({"name":"x","key":"Bad key"}),
        json!({"name":"x","key":"valid","unknown":true}),
        json!({"name":"x","key":"valid","enabled":null}),
    ] {
        assert_eq!(
            app.send("POST", "/api/v1/dictionaries", body, Some(token))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let first = dictionary(&app, token, "unique").await;
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/dictionaries",
            json!({"name":"重复","key":"unique"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let id = first["id"].as_str().unwrap();
    for body in [
        json!({"version":1,"name":null}),
        json!({"version":0,"name":"x"}),
        json!({"version":1,"description":null}),
    ] {
        assert_eq!(
            app.send(
                "PATCH",
                &format!("/api/v1/dictionaries/{id}"),
                body,
                Some(token)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let child = item(&app, token, id, "duplicate", 0).await;
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/dictionaries/{id}/items"),
            json!({"label":"Duplicate","value":"duplicate"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for body in [
        json!({"label":"x","value":""}),
        json!({"label":"x","value":"new","sort":2147483648i64}),
        json!({"label":"x","value":"new","extra":1}),
    ] {
        assert_eq!(
            app.send(
                "POST",
                &format!("/api/v1/dictionaries/{id}/items"),
                body,
                Some(token)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let second = dictionary(&app, token, "other").await;
    let second_id = second["id"].as_str().unwrap();
    item(&app, token, second_id, "duplicate", 0).await;
    assert_eq!(
        app.send(
            "PATCH",
            &format!(
                "/api/v1/dictionaries/{second_id}/items/{}",
                child["id"].as_str().unwrap()
            ),
            json!({"version":1,"label":"wrong parent"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.cleanup().await;
}

#[tokio::test]
async fn dictionary_read_only_permissions_apply_to_items_and_do_not_allow_writes() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let token = tokens["accessToken"].as_str().unwrap();
    let record = dictionary(&app, token, "access").await;
    let (status, role) = app.send("POST", "/api/v1/roles", json!({"name":"字典查看","slug":"dictionary-reader","permissions":["dictionaries.read"],"dataScope":"self"}), Some(token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = app.send("POST", "/api/v1/users", json!({"username":"dict-reader","displayName":"字典查看","password":PASSWORD,"roleIds":[role["data"]["id"]]}), Some(token)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"dict-reader","password":PASSWORD}),
            None,
        )
        .await;
    let reader = login["data"]["accessToken"].as_str().unwrap();
    let path = format!("/api/v1/dictionaries/{}", record["id"].as_str().unwrap());
    assert_eq!(
        app.send("GET", "/api/v1/dictionaries", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send("GET", &format!("{path}/items"), Value::Null, Some(reader))
            .await
            .0,
        StatusCode::OK
    );
    for (method, url, body) in [
        ("POST", "/api/v1/dictionaries".to_string(), json!({})),
        ("POST", format!("{path}/items"), json!({})),
        ("PATCH", path.clone(), json!({"version":1})),
        ("DELETE", format!("{path}?version=1"), Value::Null),
    ] {
        assert_eq!(
            app.send(method, &url, body, Some(reader)).await.0,
            StatusCode::FORBIDDEN
        );
    }
    app.cleanup().await;
}

#[tokio::test]
async fn dictionary_parent_deletion_and_item_creation_cannot_leave_an_active_orphan() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let token = tokens["accessToken"].as_str().unwrap();
    let record = dictionary(&app, token, "race").await;
    let id = record["id"].as_str().unwrap();
    let path = format!("/api/v1/dictionaries/{id}");
    let (first, second) = tokio::join!(
        app.send("PATCH", &path, json!({"version":1,"name":"A"}), Some(token)),
        app.send("PATCH", &path, json!({"version":1,"name":"B"}), Some(token))
    );
    assert_eq!(
        [first.0, second.0]
            .iter()
            .filter(|&&s| s == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [first.0, second.0]
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    let child_path = format!("{path}/items");
    let delete_path = format!("{path}?version=2");
    let (delete, create) = tokio::join!(
        app.send("DELETE", &delete_path, Value::Null, Some(token)),
        app.send(
            "POST",
            &child_path,
            json!({"label":"race","value":"race"}),
            Some(token)
        )
    );
    assert!(
        matches!(
            (delete.0, create.0),
            (StatusCode::NO_CONTENT, StatusCode::NOT_FOUND)
                | (StatusCode::CONFLICT, StatusCode::CREATED)
        ),
        "delete={delete:?}, create={create:?}"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn dictionary_schema_is_required_for_ready_and_migrations_are_idempotent() {
    let app = TestApp::new().await;
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let metadata = app.database.collection::<Document>("_schema_migrations");
    assert_eq!(
        metadata
            .count_documents(doc! {"_id":"dictionaries","version":1})
            .await
            .unwrap(),
        1
    );
    metadata
        .delete_one(doc! {"_id":"dictionaries"})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}

#[tokio::test]
async fn dictionary_item_queries_use_bounded_index_scans_on_a_populated_collection() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let token = tokens["accessToken"].as_str().unwrap();
    let record = dictionary(&app, token, "performance").await;
    let parent = mongodb::bson::oid::ObjectId::parse_str(record["id"].as_str().unwrap()).unwrap();
    let now = mongodb::bson::DateTime::now();
    let records = (0..2000).map(|index| doc! {
        "dictionaryId":parent,"label":format!("选项 {index}"),"value":format!("row{index:04}"),
        "sort":index,"enabled":true,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null
    }).collect::<Vec<_>>();
    app.database
        .collection::<Document>("dictionary_items")
        .insert_many(records)
        .await
        .unwrap();
    for (filter, sort) in [
        (
            doc! {"dictionaryId":parent,"deletedAt":null},
            doc! {"sort":1,"_id":1},
        ),
        (
            doc! {"dictionaryId":parent,"deletedAt":null,"value":{"$regex":"^row00"}},
            doc! {"value":1,"_id":1},
        ),
    ] {
        let explain = app.database.run_command(doc!{"explain":{"find":"dictionary_items","filter":filter,"sort":sort,"limit":20},"verbosity":"executionStats"}).await.unwrap();
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
            "dictionary_items: returned={}, docs={}, keys={} of 2000",
            number("nReturned"),
            number("totalDocsExamined"),
            number("totalKeysExamined")
        );
    }
    app.cleanup().await;
}
