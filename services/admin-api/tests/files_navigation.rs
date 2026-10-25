mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::Value;
use support::TestApp;
#[tokio::test]
async fn files_page_requires_schema_four_and_seeds_its_navigation_once() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let menu = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"path":"/files","viewKey":"files","deletedAt":null})
        .await
        .unwrap();
    assert!(menu.is_some(), "files page navigation was not seeded");
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"files"}, doc! {"$set":{"version":3}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    let (_, nav) = app
        .send("GET", "/api/v1/navigation", Value::Null, Some(token))
        .await;
    assert!(nav.to_string().contains("\"viewKey\":\"files\""));
    assert_eq!(
        app.database
            .collection::<Document>("menus")
            .count_documents(doc! {"path":"/files","deletedAt":null})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn files_menu_migration_preserves_customized_deleted_and_occupied_paths() {
    let app = TestApp::new().await;
    let menu = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"path":"/files","viewKey":"files"})
        .await
        .unwrap()
        .expect("missing files menu");
    let id = menu.get_object_id("_id").unwrap();
    app.database
        .collection::<Document>("menus")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"label":"素材仓库","sort":17,"version":9i64}},
        )
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"files"}, doc! {"$set":{"version":3}})
        .await
        .unwrap();
    admin_api::storage::files_schema::migrate_files(&app.database)
        .await
        .unwrap();
    let row = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_str("label").unwrap(), "素材仓库");
    assert_eq!(row.get_i64("version").unwrap(), 9);
    app.database
        .collection::<Document>("menus")
        .update_one(doc! {"_id":id}, doc! {"$set":{"deletedAt":DateTime::now()}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"files"}, doc! {"$set":{"version":3}})
        .await
        .unwrap();
    admin_api::storage::files_schema::migrate_files(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("menus")
            .count_documents(doc! {"path":"/files","deletedAt":null})
            .await
            .unwrap(),
        0
    );
    let now = DateTime::now();
    app.database.collection::<Document>("menus").insert_one(doc!{"_id":ObjectId::new(),"label":"自定义文件路径","path":"/files","viewKey":"dashboard","permission":"dashboard.read","parentId":null,"icon":null,"enabled":true,"sort":20,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).await.unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"files"}, doc! {"$set":{"version":3}})
        .await
        .unwrap();
    admin_api::storage::files_schema::migrate_files(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("menus")
            .count_documents(doc! {"path":"/files","deletedAt":null})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("menus")
            .count_documents(doc! {"path":"/files","viewKey":"files","deletedAt":null})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}
