mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

fn input(request: &str) -> Value {
    json!({"requestId":request,"name":"Automation","scopes":["parameters.read"]})
}
async fn issue(app: &TestApp, session: &str, body: Value) -> Value {
    let (status, response) = app
        .send("POST", "/api/v1/api-tokens", body, Some(session))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{response}");
    response["data"].clone()
}
#[tokio::test]
async fn api_token_is_shown_once_and_same_request_cannot_issue_twice() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let first = issue(&app, session, input(&request)).await;
    let secret = first["secret"].as_str().unwrap();
    assert!(secret.starts_with("tri_at1."));
    let id = first["token"]["id"].as_str().unwrap();
    assert_eq!(first["token"]["status"], "active");
    assert_eq!(first["token"]["minuteLimit"], 600);
    assert_eq!(first["token"]["dailyQuota"], 100000);
    let record = app
        .database
        .collection::<Document>("api_tokens")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.get_str("secretHash").unwrap().len(), 64);
    assert!(!format!("{record:?}").contains(secret));
    let (status, replay) = app
        .send("POST", "/api/v1/api-tokens", input(&request), Some(session))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["secret"], Value::Null);
    assert_eq!(replay["data"]["token"]["id"], id);
    let (_, found) = app
        .send(
            "GET",
            &format!("/api/v1/api-tokens/by-request/{request}"),
            Value::Null,
            Some(session),
        )
        .await;
    assert_eq!(found["data"]["id"], id);
    assert!(!found.to_string().contains("secretHash"));
    assert!(!found.to_string().contains(secret));
    let mut changed = input(&request);
    changed["name"] = json!("different");
    assert_eq!(
        app.send("POST", "/api/v1/api-tokens", changed, Some(session))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.database
            .collection::<Document>("api_tokens")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"apiTokens.create"})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.send("GET", "/api/v1/parameters", Value::Null, Some(secret))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/parameters",
            json!({"name":"no","key":"no","valueType":"boolean","value":true}),
            Some(secret)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        assert_eq!(
            app.send(
                "POST",
                &format!("/api/v1/api-tokens/{id}/revoke"),
                json!({}),
                Some(session)
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        app.send("GET", "/api/v1/parameters", Value::Null, Some(secret))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"apiTokens.revoke"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_issue_has_one_secret_and_invalid_scope_or_audit_failure_has_no_record() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let (one, two) = tokio::join!(
        app.send("POST", "/api/v1/api-tokens", input(&request), Some(session)),
        app.send("POST", "/api/v1/api-tokens", input(&request), Some(session))
    );
    assert!([one.0, two.0].contains(&StatusCode::CREATED));
    assert!([one.0, two.0].contains(&StatusCode::OK));
    assert_eq!(
        [&one.1, &two.1]
            .iter()
            .filter(|value| value["data"]["secret"].is_string())
            .count(),
        1
    );
    for scope in ["*", "not.a.scope", "game.commands"] {
        let mut body = input(&uuid::Uuid::new_v4().to_string());
        body["scopes"] = json!([scope]);
        assert_eq!(
            app.send("POST", "/api/v1/api-tokens", body, Some(session))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"apiTokens.create"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/api-tokens",
            input(&uuid::Uuid::new_v4().to_string()),
            Some(session)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("api_tokens")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn expired_and_revoked_credentials_cannot_use_authentication_or_mint_credentials() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let mut body = input(&uuid::Uuid::new_v4().to_string());
    body["scopes"] = json!(["apiTokens.read", "apiTokens.write", "parameters.read"]);
    let issued = issue(&app, session, body).await;
    let secret = issued["secret"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/api-tokens",
            input(&uuid::Uuid::new_v4().to_string()),
            Some(secret)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/password",
            json!({"currentPassword":PASSWORD,"newPassword":"AnotherPassword-2026"}),
            Some(secret)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let id = ObjectId::parse_str(issued["token"]["id"].as_str().unwrap()).unwrap();
    app.database
        .collection::<Document>("api_tokens")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(secret))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, read) = app
        .send(
            "GET",
            &format!("/api/v1/api-tokens/{id}"),
            Value::Null,
            Some(session),
        )
        .await;
    assert_eq!(read["data"]["status"], "expired");
    app.cleanup().await;
}

#[tokio::test]
async fn tokens_follow_live_owner_permissions_and_scope_and_other_owners_cannot_read_or_revoke_them()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let (code,role)=app.send("POST","/api/v1/roles",json!({"name":"Token role","slug":"token-role","permissions":["apiTokens.read","apiTokens.write","users.read","parameters.read"],"dataScope":"self"}),Some(owner)).await;
    assert_eq!(code, StatusCode::CREATED);
    let role_id = role["data"]["id"].as_str().unwrap();
    let (code,user)=app.send("POST","/api/v1/users",json!({"username":"token-user","displayName":"Token user","password":PASSWORD,"roleIds":[role_id]}),Some(owner)).await;
    assert_eq!(code, StatusCode::CREATED);
    let user_id = user["data"]["id"].as_str().unwrap();
    let (code, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"token-user","password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    let session = login["data"]["accessToken"].as_str().unwrap();
    let mut body = input(&uuid::Uuid::new_v4().to_string());
    body["scopes"] = json!(["users.read"]);
    let token = issue(&app, session, body).await;
    let id = token["token"]["id"].as_str().unwrap();
    let secret = token["secret"].as_str().unwrap();
    let (_, users) = app
        .send("GET", "/api/v1/users", Value::Null, Some(secret))
        .await;
    assert_eq!(users["meta"]["total"], 1);
    assert_eq!(users["data"][0]["id"], user_id);
    assert_eq!(
        app.send("GET", "/api/v1/parameters", Value::Null, Some(secret))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut beyond = input(&uuid::Uuid::new_v4().to_string());
    beyond["scopes"] = json!(["parameters.write"]);
    assert_eq!(
        app.send("POST", "/api/v1/api-tokens", beyond, Some(session))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for path in [
        format!("/api/v1/api-tokens/{id}"),
        format!(
            "/api/v1/api-tokens/by-request/{}",
            token["token"]["requestId"].as_str().unwrap()
        ),
    ] {
        assert_eq!(
            app.send("GET", &path, Value::Null, Some(owner)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/api-tokens/{id}/revoke"),
            json!({}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"_id":ObjectId::parse_str(role_id).unwrap()},
            doc! {"$set":{"dataScope":"all"}},
        )
        .await
        .unwrap();
    let (_, users) = app
        .send("GET", "/api/v1/users", Value::Null, Some(secret))
        .await;
    assert_eq!(users["meta"]["total"], 2);
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"_id":ObjectId::parse_str(role_id).unwrap()},
            doc! {"$pull":{"permissions":"users.read"}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/users", Value::Null, Some(secret))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/users/{user_id}/password"),
            json!({"version":1,"password":"ResetTokenPassword-2026"}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let raw = app
        .database
        .collection::<Document>("api_tokens")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert!(raw.get_datetime("revokedAt").is_ok());
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(secret))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"token-user","password":"ResetTokenPassword-2026"}),
            None,
        )
        .await;
    let current = login["data"]["accessToken"].as_str().unwrap();
    let another = issue(&app, current, input(&uuid::Uuid::new_v4().to_string())).await;
    let live = another["secret"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/parameters", Value::Null, Some(live))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{user_id}"),
            json!({"version":2,"status":"disabled"}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("GET", "/api/v1/parameters", Value::Null, Some(live))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn issuance_validates_boundaries_and_own_schema_replay_preserves_credentials() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    for (key, value) in [
        ("name", json!("\n")),
        ("expiresInDays", json!(0)),
        ("expiresInDays", json!(366)),
        ("minuteLimit", json!(0)),
        ("minuteLimit", json!(10001)),
        ("dailyQuota", json!(0)),
        ("dailyQuota", json!(10000001)),
        ("allowedIps", json!(["localhost"])),
        ("scopes", json!(["parameters.read", "parameters.read"])),
        ("requestId", json!("not-uuid")),
        ("unexpected", json!(true)),
    ] {
        let mut body = input(&uuid::Uuid::new_v4().to_string());
        body[key] = value;
        assert_eq!(
            app.send("POST", "/api/v1/api-tokens", body, Some(session))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{key}"
        );
    }
    let token = issue(&app, session, input(&uuid::Uuid::new_v4().to_string())).await;
    let id = ObjectId::parse_str(token["token"]["id"].as_str().unwrap()).unwrap();
    let before = app
        .database
        .collection::<Document>("api_tokens")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        before,
        app.database
            .collection::<Document>("api_tokens")
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap()
    );
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"open-access"}, doc! {"$set":{"version":999}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .is_err()
    );
    app.cleanup().await;
}

#[tokio::test]
async fn thirty_two_active_tokens_are_bounded_and_replay_expiry_and_revocation_release_capacity() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let first = issue(&app, session, input(&request)).await;
    for _ in 1..31 {
        issue(&app, session, input(&uuid::Uuid::new_v4().to_string())).await;
    }
    let (one, two) = tokio::join!(
        app.send(
            "POST",
            "/api/v1/api-tokens",
            input(&uuid::Uuid::new_v4().to_string()),
            Some(session)
        ),
        app.send(
            "POST",
            "/api/v1/api-tokens",
            input(&uuid::Uuid::new_v4().to_string()),
            Some(session)
        )
    );
    assert!([one.0, two.0].contains(&StatusCode::CREATED));
    assert!([one.0, two.0].contains(&StatusCode::TOO_MANY_REQUESTS));
    assert_eq!(
        app.send("POST", "/api/v1/api-tokens", input(&request), Some(session))
            .await
            .0,
        StatusCode::OK
    );
    let first_id = first["token"]["id"].as_str().unwrap();
    app.database
        .collection::<Document>("api_tokens")
        .update_one(
            doc! {"_id":ObjectId::parse_str(first_id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let new = issue(&app, session, input(&uuid::Uuid::new_v4().to_string())).await;
    let new_id = new["token"]["id"].as_str().unwrap();
    app.send(
        "POST",
        &format!("/api/v1/api-tokens/{new_id}/revoke"),
        json!({}),
        Some(session),
    )
    .await;
    issue(&app, session, input(&uuid::Uuid::new_v4().to_string())).await;
    let (_, page) = app
        .send(
            "GET",
            "/api/v1/api-tokens?page=2&pageSize=20",
            Value::Null,
            Some(session),
        )
        .await;
    assert_eq!(page["meta"]["total"], 34);
    assert_eq!(page["data"].as_array().unwrap().len(), 14);
    app.cleanup().await;
}

#[tokio::test]
async fn scoped_tokens_cannot_escape_their_limits_by_changing_administrator_accounts() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let owner = auth["user"]["id"].as_str().unwrap();
    let mut body = input(&uuid::Uuid::new_v4().to_string());
    body["scopes"] = json!([
        "users.read",
        "users.write",
        "users.resetPassword",
        "roles.assign"
    ]);
    let token = issue(&app, session, body).await;
    let secret = token["secret"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/users/{owner}/password"),
            json!({"version":1,"password":"MustNotAcquireAdmin-2026"}),
            Some(secret)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_,role)=app.send("POST","/api/v1/roles",json!({"name":"Basic","slug":"pat-basic","permissions":["users.read"],"dataScope":"self"}),Some(session)).await;
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"must-not-mint-session","displayName":"Denied","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(secret)).await.0,StatusCode::FORBIDDEN);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/users/{owner}"),
            json!({"version":1,"displayName":"Denied"}),
            Some(secret)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/users/{owner}?version=1"),
            Value::Null,
            Some(secret)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}
