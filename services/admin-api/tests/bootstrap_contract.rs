use admin_api::bootstrap::{AdminInput,bootstrap_database};
use mongodb::{Client,Database,bson::{doc,Document}};

async fn isolated_database() -> Database {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env");
    let values: std::collections::HashMap<_,_> = dotenvy::from_path_iter(root).unwrap().map(Result::unwrap).collect();
    let url = values.get("TRI_MONGODB_URI").expect("TRI_MONGODB_URI is required for isolated integration tests");
    let client = Client::with_uri_str(url).await.unwrap();
    let name = format!("tri_admin_test_{}",uuid::Uuid::new_v4().simple());
    client.database(&name)
}

#[tokio::test]
async fn bootstrap_is_idempotent_and_never_resets_an_existing_password() {
    let database = isolated_database().await;
    let first = AdminInput{username:"admin".into(),password:"fixture-initial-password".into()};
    let report = bootstrap_database(&database,&first).await.unwrap();
    assert!(report.admin_created);
    let users = database.collection::<Document>("sys_users");
    let user = users.find_one(doc!{"username":"admin"}).await.unwrap().unwrap();
    let hash = user.get_str("password").unwrap().to_owned();
    assert!(bcrypt::verify("fixture-initial-password",&hash).unwrap());
    let second = AdminInput{username:"admin".into(),password:"different-password-must-not-replace".into()};
    assert!(!bootstrap_database(&database,&second).await.unwrap().admin_created);
    let stored = users.find_one(doc!{"username":"admin"}).await.unwrap().unwrap();
    assert_eq!(stored.get_str("password").unwrap(),hash);
    assert_eq!(users.count_documents(doc!{"username":"admin"}).await.unwrap(),1);
    database.drop().await.unwrap();
}

#[tokio::test]
async fn concurrent_bootstrap_creates_one_administrator_with_consistent_role() {
    let database = isolated_database().await;
    let input = AdminInput{username:"admin".into(),password:"fixture-initial-password".into()};
    let (first,second) = tokio::join!(bootstrap_database(&database,&input),bootstrap_database(&database,&input));
    assert_ne!(first.unwrap().admin_created,second.unwrap().admin_created);
    assert_eq!(database.collection::<Document>("sys_users").count_documents(doc!{}).await.unwrap(),1);
    let user = database.collection::<Document>("sys_users").find_one(doc!{}).await.unwrap().unwrap();
    assert_eq!(user.get_array("authority_ids").unwrap(),&vec![mongodb::bson::Bson::Int64(888)]);
    database.drop().await.unwrap();
}

#[tokio::test]
async fn unsupported_schema_version_is_rejected_without_creating_an_admin() {
    let database = isolated_database().await;
    database.collection::<Document>("_schema_migrations").insert_one(doc!{"_id":"tri-admin","version":999}).await.unwrap();
    let input = AdminInput{username:"admin".into(),password:"fixture-initial-password".into()};
    let result = bootstrap_database(&database,&input).await;
    assert!(result.is_err());
    assert_eq!(database.collection::<Document>("sys_users").count_documents(doc!{}).await.unwrap(),0);
    database.drop().await.unwrap();
}
