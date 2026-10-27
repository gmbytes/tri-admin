mod support;
use mongodb::{
    IndexModel,
    bson::{DateTime, Document, doc, oid::ObjectId},
    options::IndexOptions,
};
use support::TestApp;

#[tokio::test]
async fn own_v1_upgrade_preserves_stdio_identity_history_and_sealed_results() {
    let app = TestApp::new().await;
    let _ = app.login().await;
    let db = &app.database;
    let owner = ObjectId::new();
    let connection = ObjectId::new();
    let instance = ObjectId::new();
    let action = ObjectId::new();
    let now = DateTime::now();
    db.collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"mcp"}, doc! {"$set":{"version":1i32}})
        .await
        .unwrap();
    db.collection::<Document>("mcp_connections").insert_one(doc!{"_id":connection,"ownerId":owner,"requestId":"v1-connection","name":"preserved","transport":{"kind":"stdio","capabilityId":"fixture"},"runtime":{"id":instance,"serviceId":"old-service","state":"running","pid":321i64,"exitConfirmed":false,"leaseUntil":now,"updatedAt":now,"errorCode":null},"deletedAt":null}).await.unwrap();
    db.collection::<Document>("mcp_instances").insert_one(doc!{"_id":instance,"connectionId":connection,"state":"running","pid":321i64,"exitConfirmed":false,"toolsJson":"[]"}).await.unwrap();
    let old_action = doc! {"_id":action,"actorId":owner,"requestId":"original-uuid-receipt","connectionId":connection,"instanceId":instance,"status":"unknown","argumentsSha256":"preserved-args-hash"};
    let old_result = doc! {"_id":action,"sealed":{"keyId":"fixture-old","nonce":"preserved-nonce","ciphertext":"preserved-ciphertext"},"expiresAt":DateTime::from_millis(now.timestamp_millis()+86400000)};
    db.collection::<Document>("mcp_actions")
        .insert_one(old_action.clone())
        .await
        .unwrap();
    db.collection::<Document>("mcp_results")
        .insert_one(old_result.clone())
        .await
        .unwrap();
    db.collection::<Document>("mcp_connections")
        .create_index(
            IndexModel::builder()
                .keys(doc! {"name":1,"ownerId":1})
                .options(
                    IndexOptions::builder()
                        .name("custom_mcp_fixture".to_owned())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    admin_api::storage::mcp_schema::migrate(db).await.unwrap();
    let marker = db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"mcp"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(marker.get_i32("version").unwrap(), 2);
    let row = db
        .collection::<Document>("mcp_connections")
        .find_one(doc! {"_id":connection})
        .await
        .unwrap()
        .unwrap();
    let runtime = row.get_document("runtime").unwrap();
    assert_eq!(runtime.get_str("transportKind").unwrap(), "stdio");
    assert!(!runtime.get_bool("releaseConfirmed").unwrap());
    assert!(!runtime.contains_key("exitConfirmed"));
    assert_eq!(runtime.get_i64("pid").unwrap(), 321);
    let row = db
        .collection::<Document>("mcp_instances")
        .find_one(doc! {"_id":instance})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_str("transportKind").unwrap(), "stdio");
    assert!(!row.get_bool("releaseConfirmed").unwrap());
    assert!(!row.contains_key("exitConfirmed"));
    assert_eq!(
        db.collection::<Document>("mcp_actions")
            .find_one(doc! {"_id":action})
            .await
            .unwrap()
            .unwrap(),
        old_action
    );
    assert_eq!(
        db.collection::<Document>("mcp_results")
            .find_one(doc! {"_id":action})
            .await
            .unwrap()
            .unwrap(),
        old_result
    );
    db.collection::<Document>("mcp_instances")
        .update_one(
            doc! {"_id":instance},
            doc! {"$set":{"transportKind":"http","releaseConfirmed":true,"pid":null}},
        )
        .await
        .unwrap();
    admin_api::storage::mcp_schema::migrate(db).await.unwrap();
    let row = db
        .collection::<Document>("mcp_instances")
        .find_one(doc! {"_id":instance})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_str("transportKind").unwrap(), "http");
    assert!(row.get_bool("releaseConfirmed").unwrap());
    let indexes = db
        .collection::<Document>("mcp_connections")
        .list_index_names()
        .await
        .unwrap();
    assert!(indexes.iter().any(|name| name == "custom_mcp_fixture"));
    db.collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"mcp"}, doc! {"$set":{"version":999i32}})
        .await
        .unwrap();
    assert!(admin_api::storage::mcp_schema::migrate(db).await.is_err());
    assert_eq!(
        db.collection::<Document>("mcp_instances")
            .find_one(doc! {"_id":instance})
            .await
            .unwrap()
            .unwrap(),
        row
    );
    app.cleanup().await;
}
