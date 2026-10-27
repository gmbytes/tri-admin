mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::Value;
use sha2::{Digest, Sha256};
use support::TestApp;
fn seeded() -> ObjectId {
    let hash = Sha256::digest("tri-admin:menu:/mcp");
    let mut bytes = [0; 12];
    bytes.copy_from_slice(&hash[..12]);
    ObjectId::from_bytes(bytes)
}
fn has_mcp(value: &Value) -> bool {
    value.as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["viewKey"] == "mcp" || has_mcp(&row["children"]))
    })
}
#[tokio::test]
async fn own_v2_navigation_upgrade_adds_mcp_once_and_preserves_occupied_custom_and_deleted_entries()
{
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (status, nav) = app
        .send("GET", "/api/v1/navigation", Value::Null, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_mcp(&nav["data"]));
    for scenario in ["absent", "occupied", "deleted", "custom"] {
        app.database
            .collection::<Document>("menus")
            .delete_many(doc! {"$or":[{"_id":seeded()},{"path":"/mcp"},{"path":"/custom-mcp"}]})
            .await
            .unwrap();
        app.database
            .collection::<Document>("_schema_migrations")
            .update_one(doc! {"_id":"mcp"}, doc! {"$set":{"version":2i32}})
            .await
            .unwrap();
        let mut original = None;
        if scenario != "absent" {
            let now = DateTime::now();
            let row = doc! {"_id":if scenario=="occupied"{ObjectId::new()}else{seeded()},"label":"operator value","path":if scenario=="custom"{"/custom-mcp"}else{"/mcp"},"viewKey":"mcp","permission":"mcp.read","icon":null,"parentId":null,"sort":9,"enabled":false,"version":7i64,"createdAt":now,"updatedAt":now,"deletedAt":if scenario=="deleted"{mongodb::bson::Bson::DateTime(now)}else{mongodb::bson::Bson::Null}};
            app.database
                .collection::<Document>("menus")
                .insert_one(row.clone())
                .await
                .unwrap();
            original = Some(row);
        }
        admin_api::storage::mcp_schema::migrate(&app.database)
            .await
            .unwrap();
        admin_api::storage::mcp_schema::migrate(&app.database)
            .await
            .unwrap();
        if let Some(row) = original {
            let current = app
                .database
                .collection::<Document>("menus")
                .find_one(doc! {"_id":row.get_object_id("_id").unwrap()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(current, row)
        } else {
            let row = app
                .database
                .collection::<Document>("menus")
                .find_one(doc! {"_id":seeded()})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.get_str("label").unwrap(), "MCP连接");
            assert_eq!(row.get_str("permission").unwrap(), "mcp.read");
        }
        assert_eq!(
            app.database
                .collection::<Document>("menus")
                .count_documents(doc! {"viewKey":"mcp"})
                .await
                .unwrap(),
            1
        );
    }
    app.cleanup().await;
}
