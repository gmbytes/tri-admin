use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::{
    Client,
    bson::{Document, doc, oid::ObjectId},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use tri_resource_runtime::{Config, State, router, schema::Schema};
const PASSWORD: &str = "GeneratedFixturePassword-2026";
struct App {
    state: State,
    router: Router,
}
impl App {
    async fn new() -> Self {
        let env =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.env")).unwrap();
        let uri = env
            .lines()
            .find_map(|line| line.strip_prefix("TRI_MONGODB_URI="))
            .unwrap();
        let database = Client::with_uri_str(uri).await.unwrap().database(&format!(
            "tri_resource_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let schema=Schema::parse(json!({"resource":"inventory","label":"库存","fields":[{"name":"sku","label":"编号","kind":"text","required":true,"searchable":true,"unique":true},{"name":"count","label":"数量","kind":"integer","required":true}]})).unwrap();
        let state = State::new(
            database,
            schema,
            Config::new(
                "fixture-resource-signing-key-at-least-32-bytes".into(),
                "http://localhost:5173".into(),
            )
            .unwrap(),
        )
        .unwrap();
        state.migrate().await.unwrap();
        state
            .provision_admin("owner", PASSWORD, true, true)
            .await
            .unwrap();
        Self {
            router: router(state.clone()),
            state,
        }
    }
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Value,
        token: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .router
            .clone()
            .oneshot(
                request
                    .body(if body.is_null() {
                        Body::empty()
                    } else {
                        Body::from(body.to_string())
                    })
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            },
        )
    }
    async fn login(&self, name: &str) -> String {
        let (status, value) = self
            .send(
                "POST",
                "/api/v1/auth/login",
                json!({"username":name,"password":PASSWORD}),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        value["data"]["accessToken"].as_str().unwrap().into()
    }
    async fn cleanup(self) {
        self.state.database.drop().await.unwrap();
    }
}
#[tokio::test]
async fn generated_resource_real_crud_is_versioned_audited_and_private() {
    let app = App::new().await;
    let token = app.login("owner").await;
    assert_eq!(
        app.send("GET", "/api/v1/inventory", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, created) = app
        .send(
            "POST",
            "/api/v1/inventory",
            json!({"values":{"sku":"do-not-log-value","count":7}}),
            Some(&token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["data"]["id"].as_str().unwrap();
    assert_eq!(created["data"]["version"], 1);
    assert!(!created.to_string().contains("password"));
    assert!(!created.to_string().contains("$oid"));
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/inventory",
            json!({"values":{"sku":"do-not-log-value","count":7}}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, list) = app
        .send(
            "GET",
            "/api/v1/inventory?page=1&pageSize=1&search=do-not",
            Value::Null,
            Some(&token),
        )
        .await;
    assert_eq!(list["meta"]["total"], 1);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/inventory/{id}"),
            json!({"version":1,"values":{"sku":"updated","count":8}}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/inventory/{id}"),
            json!({"version":1,"values":{"sku":"stale","count":8}}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/inventory/{id}?version=2"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/inventory/{id}"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.state
            .database
            .collection::<Document>("_resource_audit")
            .count_documents(doc! {"action":{"$in":["create","update","delete"]}})
            .await
            .unwrap(),
        3
    );
    let mut cursor = app
        .state
        .database
        .collection::<Document>("_resource_audit")
        .find(doc! {})
        .await
        .unwrap();
    use futures_util::TryStreamExt;
    while let Some(row) = cursor.try_next().await.unwrap() {
        assert!(!format!("{row:?}").contains("do-not-log-value"));
        assert!(!format!("{row:?}").contains(PASSWORD));
    }
    app.cleanup().await;
}
#[tokio::test]
async fn self_scope_read_only_and_live_session_revocation_are_enforced() {
    let app = App::new().await;
    let alice = app
        .state
        .provision_admin("alice", PASSWORD, true, false)
        .await
        .unwrap();
    app.state
        .provision_admin("bob", PASSWORD, true, false)
        .await
        .unwrap();
    app.state
        .provision_admin("reader", PASSWORD, false, true)
        .await
        .unwrap();
    let a = app.login("alice").await;
    let b = app.login("bob").await;
    let r = app.login("reader").await;
    let (_, first) = app
        .send(
            "POST",
            "/api/v1/inventory",
            json!({"values":{"sku":"same","count":1}}),
            Some(&a),
        )
        .await;
    let id = first["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/inventory",
            json!({"values":{"sku":"same","count":2}}),
            Some(&b)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/inventory/{id}"),
            Value::Null,
            Some(&b)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/inventory/{id}"),
            json!({"version":1,"values":{"sku":"other","count":2}}),
            Some(&b)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, list) = app
        .send("GET", "/api/v1/inventory", Value::Null, Some(&a))
        .await;
    assert_eq!(list["meta"]["total"], 1);
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/inventory",
            json!({"values":{"sku":"blocked","count":1}}),
            Some(&r)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/inventory",
            json!({"ownerId":alice.id,"values":{"sku":"injected","count":1}}),
            Some(&b)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.state
        .database
        .collection::<Document>("_resource_admins")
        .update_one(
            doc! {"_id":ObjectId::parse_str(&alice.id).unwrap()},
            doc! {"$set":{"enabled":false}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/inventory", Value::Null, Some(&a))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send("POST", "/api/v1/auth/logout", json!({}), Some(&b))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("GET", "/api/v1/inventory", Value::Null, Some(&b))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}
#[tokio::test]
async fn audit_failure_rolls_back_and_schema_changes_are_not_silently_applied() {
    let app = App::new().await;
    let token = app.login("owner").await;
    app.state.database.run_command(doc!{"collMod":"_resource_audit","validator":{"mustNeverExist":{"$exists":true}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/inventory",
            json!({"values":{"sku":"rollback","count":1}}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        app.state
            .database
            .collection::<Document>("records_inventory")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let mut schema = app.state.schema.as_ref().clone();
    schema.fields[0].label = "changed".into();
    let other = State::new(
        app.state.database.clone(),
        schema,
        Config::new(
            "fixture-resource-signing-key-at-least-32-bytes".into(),
            "http://localhost:5173".into(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(other.migrate().await.is_err());
    app.cleanup().await;
}

#[tokio::test]
async fn absent_or_mismatched_schema_fails_closed_and_never_adopts_existing_records() {
    let app = App::new().await;
    let token = app.login("owner").await;
    app.state
        .database
        .collection::<Document>("_resource_schema")
        .delete_one(doc! {"_id":"inventory"})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/inventory", Value::Null, Some(&token))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.state
        .database
        .collection::<Document>("records_inventory")
        .insert_one(doc! {"existing":"unmanaged"})
        .await
        .unwrap();
    assert!(app.state.migrate().await.is_err());
    assert_eq!(
        app.state
            .database
            .collection::<Document>("records_inventory")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn concurrent_migrations_cannot_both_accept_different_descriptors() {
    let app = App::new().await;
    let mut first = app.state.schema.as_ref().clone();
    first.resource = "concurrent".into();
    let mut second = first.clone();
    second.label = "other descriptor".into();
    let a = State::new(
        app.state.database.clone(),
        first,
        Config::new(
            "fixture-resource-signing-key-at-least-32-bytes".into(),
            "http://localhost:5173".into(),
        )
        .unwrap(),
    )
    .unwrap();
    let b = State::new(
        app.state.database.clone(),
        second,
        Config::new(
            "fixture-resource-signing-key-at-least-32-bytes".into(),
            "http://localhost:5173".into(),
        )
        .unwrap(),
    )
    .unwrap();
    let (left, right) = tokio::join!(a.migrate(), b.migrate());
    assert_ne!(left.is_ok(), right.is_ok());
    app.cleanup().await;
}
#[tokio::test]
async fn concurrent_updates_and_live_permission_shrink_have_one_authoritative_result() {
    let app = App::new().await;
    let token = app.login("owner").await;
    let (_, created) = app
        .send(
            "POST",
            "/api/v1/inventory",
            json!({"values":{"sku":"concurrent","count":1}}),
            Some(&token),
        )
        .await;
    let id = created["data"]["id"].as_str().unwrap();
    let path = format!("/api/v1/inventory/{id}");
    let (left, right) = tokio::join!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"values":{"sku":"left","count":2}}),
            Some(&token)
        ),
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"values":{"sku":"right","count":3}}),
            Some(&token)
        )
    );
    assert_eq!(
        [left.0, right.0]
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [left.0, right.0]
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    app.state
        .database
        .collection::<Document>("_resource_admins")
        .update_one(
            doc! {"usernameKey":"owner"},
            doc! {"$set":{"canWrite":false}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":2,"values":{"sku":"blocked","count":4}}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, view) = app
        .send("GET", "/api/v1/auth/me", Value::Null, Some(&token))
        .await;
    assert_eq!(view["data"]["canWrite"], false);
    app.cleanup().await;
}
#[tokio::test]
async fn concurrent_login_limits_are_atomic_and_do_not_create_untracked_sessions() {
    let app = App::new().await;
    let results = futures_util::future::join_all((0..12).map(|_| {
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"owner","password":PASSWORD}),
            None,
        )
    }))
    .await;
    let accepted = results.iter().filter(|row| row.0 == StatusCode::OK).count();
    assert_eq!(accepted, 10);
    assert!(
        results
            .iter()
            .all(|row| row.0 == StatusCode::OK || row.0 == StatusCode::TOO_MANY_REQUESTS)
    );
    assert_eq!(
        app.state
            .database
            .collection::<Document>("_resource_sessions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        accepted as u64
    );
    app.cleanup().await;
}
