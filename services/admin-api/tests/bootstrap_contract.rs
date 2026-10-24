use admin_api::bootstrap::{AdminInput, bootstrap_database};
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use mongodb::{
    Client, Database,
    bson::{Document, doc},
};

async fn isolated_database() -> Database {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env");
    let values: std::collections::HashMap<_, _> = dotenvy::from_path_iter(root)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let url = values
        .get("TRI_MONGODB_URI")
        .expect("TRI_MONGODB_URI is required for isolated integration tests");
    let client = Client::with_uri_str(url).await.unwrap();
    let name = format!("tri_admin_test_{}", uuid::Uuid::new_v4().simple());
    client.database(&name)
}

#[tokio::test]
async fn bootstrap_is_idempotent_and_never_resets_an_existing_password() {
    let database = isolated_database().await;
    let first = AdminInput {
        username: "admin".into(),
        password: "fixture-initial-password".into(),
    };
    let report = bootstrap_database(&database, &first).await.unwrap();
    assert!(report.admin_created);
    let users = database.collection::<Document>("users");
    let user = users
        .find_one(doc! {"username":"admin"})
        .await
        .unwrap()
        .unwrap();
    let hash = user.get_str("passwordHash").unwrap().to_owned();
    assert!(
        Argon2::default()
            .verify_password(
                b"fixture-initial-password",
                &PasswordHash::new(&hash).unwrap()
            )
            .is_ok()
    );
    assert!(mongodb::bson::oid::ObjectId::parse_str(&report.user_id).is_ok());
    let second = AdminInput {
        username: "admin".into(),
        password: "different-password-must-not-replace".into(),
    };
    assert!(
        !bootstrap_database(&database, &second)
            .await
            .unwrap()
            .admin_created
    );
    let stored = users
        .find_one(doc! {"username":"admin"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.get_str("passwordHash").unwrap(), hash);
    assert_eq!(
        users
            .count_documents(doc! {"username":"admin"})
            .await
            .unwrap(),
        1
    );
    database.drop().await.unwrap();
}

#[tokio::test]
async fn concurrent_bootstrap_creates_one_administrator_with_consistent_role() {
    let database = isolated_database().await;
    let input = AdminInput {
        username: "admin".into(),
        password: "fixture-initial-password".into(),
    };
    let (first, second) = tokio::join!(
        bootstrap_database(&database, &input),
        bootstrap_database(&database, &input)
    );
    assert_ne!(first.unwrap().admin_created, second.unwrap().admin_created);
    assert_eq!(
        database
            .collection::<Document>("users")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let user = database
        .collection::<Document>("users")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let role = database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"owner"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        user.get_array("roleIds").unwrap(),
        &vec![role.get("_id").unwrap().clone()]
    );
    database.drop().await.unwrap();
}

#[tokio::test]
async fn unsupported_schema_version_is_rejected_without_creating_an_admin() {
    let database = isolated_database().await;
    database
        .collection::<Document>("_schema_migrations")
        .insert_one(doc! {"_id":"core","version":999})
        .await
        .unwrap();
    let input = AdminInput {
        username: "admin".into(),
        password: "fixture-initial-password".into(),
    };
    let result = bootstrap_database(&database, &input).await;
    assert!(result.is_err());
    assert_eq!(
        database
            .collection::<Document>("users")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    database.drop().await.unwrap();
}

#[tokio::test]
async fn failed_user_insert_rolls_back_owner_role_and_initialization_marker() {
    let database = isolated_database().await;
    admin_api::storage::schema::migrate_database(&database)
        .await
        .unwrap();
    database.run_command(doc!{"collMod":"users","validator":{"$jsonSchema":{"bsonType":"object","required":["fixtureMustNotExist"]}}}).await.unwrap();
    let input = AdminInput {
        username: "admin".into(),
        password: "fixture-initial-password".into(),
    };
    assert!(bootstrap_database(&database, &input).await.is_err());
    assert_eq!(
        database
            .collection::<Document>("roles")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let schema = database
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"core"})
        .await
        .unwrap()
        .unwrap();
    assert!(!schema.get_bool("bootstrapComplete").unwrap());
    database.drop().await.unwrap();
}

#[tokio::test]
async fn refresh_token_hash_is_unique_and_deleted_menu_routes_can_be_reused() {
    let database = isolated_database().await;
    admin_api::storage::schema::migrate_database(&database)
        .await
        .unwrap();
    let sessions = database.collection::<Document>("sessions");
    sessions
        .insert_one(doc! {"refreshHash":"fixture-hash"})
        .await
        .unwrap();
    assert!(
        sessions
            .insert_one(doc! {"refreshHash":"fixture-hash"})
            .await
            .is_err()
    );
    let menus = database.collection::<Document>("menus");
    menus
        .insert_one(doc! {"path":"/fixture-users","deletedAt":null})
        .await
        .unwrap();
    assert!(
        menus
            .insert_one(doc! {"path":"/fixture-users","deletedAt":null})
            .await
            .is_err()
    );
    menus
        .update_one(
            doc! {"path":"/fixture-users"},
            doc! {"$set":{"deletedAt":mongodb::bson::DateTime::now()}},
        )
        .await
        .unwrap();
    menus
        .insert_one(doc! {"path":"/fixture-users","deletedAt":null})
        .await
        .unwrap();
    database.drop().await.unwrap();
}

#[tokio::test]
async fn readiness_requires_committed_bootstrap_and_disabled_admin_is_not_reenabled() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let database = isolated_database().await;
    let config = admin_api::config::Config::from_lookup(|name| {
        (name == "TRI_JWT_SIGNING_KEY").then(|| "fixture-key-which-is-long-enough-for-tests".into())
    })
    .unwrap();
    let router = admin_api::build_router(admin_api::state::AppState::new(
        config,
        Some(database.clone()),
    ));
    let before = router
        .clone()
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(before.status(), 503);
    let input = AdminInput {
        username: "admin".into(),
        password: "fixture-initial-password".into(),
    };
    bootstrap_database(&database, &input).await.unwrap();
    let after = router
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(after.status(), 200);
    let users = database.collection::<Document>("users");
    users
        .update_one(
            doc! {"username":"admin"},
            doc! {"$set":{"status":"disabled"}},
        )
        .await
        .unwrap();
    assert!(bootstrap_database(&database, &input).await.is_err());
    let user = users
        .find_one(doc! {"username":"admin"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.get_str("status").unwrap(), "disabled");
    database.drop().await.unwrap();
}
