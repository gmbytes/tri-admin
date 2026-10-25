mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

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
