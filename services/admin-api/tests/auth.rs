mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc};
use serde_json::{Value, json};
use support::{KEY, PASSWORD, TestApp};

#[tokio::test]
async fn login_returns_new_user_and_tokens_without_exposing_password_material() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    assert_eq!(tokens["expiresIn"], 900);
    assert_eq!(tokens["user"]["username"], "admin");
    assert_eq!(tokens["user"]["permissions"], json!(["*"]));
    assert!(tokens["user"].get("passwordHash").is_none());
    let access = tokens["accessToken"].as_str().unwrap();
    assert_eq!(access.split('.').count(), 3);
    let refresh = tokens["refreshToken"].as_str().unwrap();
    assert_eq!(refresh.len(), 64);
    let session = app
        .database
        .collection::<Document>("sessions")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_ne!(session.get_str("refreshHash").unwrap(), refresh);
    let (status, me) = app
        .send("GET", "/api/v1/auth/me", Value::Null, Some(access))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["data"]["id"], tokens["user"]["id"]);
    app.cleanup().await;
}

#[tokio::test]
async fn logout_revokes_access_and_refresh_tokens() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let access = tokens["accessToken"].as_str().unwrap();
    let (status, _) = app
        .send("POST", "/api/v1/auth/logout", Value::Null, Some(access))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/refresh",
            json!({"refreshToken":tokens["refreshToken"]}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn refresh_rotation_detects_reuse_and_revokes_the_session_family() {
    let app = TestApp::new().await;
    let initial = app.login().await;
    let (status, rotated) = app
        .send(
            "POST",
            "/api/v1/auth/refresh",
            json!({"refreshToken":initial["refreshToken"]}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(initial["refreshToken"], rotated["data"]["refreshToken"]);
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/refresh",
            json!({"refreshToken":initial["refreshToken"]}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let access = rotated["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn disabled_users_and_expired_database_sessions_are_rejected_immediately() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let access = tokens["accessToken"].as_str().unwrap();
    app.database
        .collection::<Document>("users")
        .update_one(
            doc! {"username":"admin"},
            doc! {"$set":{"status":"disabled"}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"admin","password":PASSWORD}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.database
        .collection::<Document>("users")
        .update_one(doc! {"username":"admin"}, doc! {"$set":{"status":"active"}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("sessions")
        .update_many(
            doc! {},
            doc! {"$set":{"expiresAt":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn jwt_validation_rejects_expiry_wrong_audience_issuer_and_signature() {
    let app = TestApp::new().await;
    let tokens = app.login().await;
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.set_audience(&["tri-admin-desktop"]);
    validation.set_issuer(&["tri-admin"]);
    let claims = jsonwebtoken::decode::<Value>(
        tokens["accessToken"].as_str().unwrap(),
        &jsonwebtoken::DecodingKey::from_secret(KEY.as_bytes()),
        &validation,
    )
    .unwrap()
    .claims;
    for (field, value) in [
        ("exp", json!(1)),
        ("aud", json!("other-app")),
        ("iss", json!("other-issuer")),
    ] {
        let mut invalid = claims.clone();
        invalid[field] = value;
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
            &invalid,
            &jsonwebtoken::EncodingKey::from_secret(KEY.as_bytes()),
        )
        .unwrap();
        assert_eq!(
            app.send("GET", "/api/v1/auth/me", Value::Null, Some(&token))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let wrong = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"wrong-signing-key-but-long-enough-for-hs256"),
    )
    .unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(&wrong))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn invalid_credentials_are_generic_and_login_rate_is_bounded() {
    let app = TestApp::new().await;
    for _ in 0..10 {
        let (status, body) = app
            .send(
                "POST",
                "/api/v1/auth/login",
                json!({"username":"admin","password":"wrong-password"}),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "INVALID_CREDENTIALS");
    }
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"admin","password":PASSWORD}),
            None
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        app.send("POST", "/api/v1/auth/login", json!({"username":null}), None)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.cleanup().await;
}

#[tokio::test]
async fn authentication_and_readiness_wait_for_the_auth_schema() {
    let app = TestApp::new().await;
    app.database
        .collection::<Document>("_schema_migrations")
        .delete_one(doc! {"_id":"auth"})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"admin","password":PASSWORD}),
            None
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}

#[tokio::test]
async fn concurrent_refresh_reuse_revokes_the_successful_response_session() {
    let app = TestApp::new().await;
    let initial = app.login().await;
    let payload = json!({"refreshToken":initial["refreshToken"]});
    let (first, second) = tokio::join!(
        app.send("POST", "/api/v1/auth/refresh", payload.clone(), None),
        app.send("POST", "/api/v1/auth/refresh", payload, None)
    );
    let (success, failure) = if first.0 == StatusCode::OK {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(success.0, StatusCode::OK);
    assert_eq!(failure.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/auth/me",
            Value::Null,
            Some(success.1["data"]["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn current_permissions_are_reloaded_instead_of_trusting_token_role_claims() {
    let app = TestApp::new().await;
    let initial = app.login().await;
    app.database
        .collection::<Document>("roles")
        .update_one(
            doc! {"slug":"owner"},
            doc! {"$set":{"permissions":["users.read"]}},
        )
        .await
        .unwrap();
    let (status, me) = app
        .send(
            "GET",
            "/api/v1/auth/me",
            Value::Null,
            Some(initial["accessToken"].as_str().unwrap()),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["data"]["permissions"], json!(["users.read"]));
    app.cleanup().await;
}

#[tokio::test]
async fn changing_usernames_does_not_bypass_the_per_ip_login_limit() {
    let app = TestApp::with_ip_limit(2).await;
    for username in ["unknown-one", "unknown-two"] {
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/auth/login",
                json!({"username":username,"password":"wrong-password"}),
                None
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"unknown-three","password":"wrong-password"}),
            None
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    app.cleanup().await;
}
