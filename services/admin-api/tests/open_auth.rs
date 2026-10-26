mod support;
use admin_api::{build_router, config::Config, state::AppState};
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
const OPEN_KEY: &str = "open-fixture-signing-key-is-separate-from-admin-2026";
async fn fixture() -> (TestApp, String, String, String) {
    let mut app = TestApp::new().await;
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_OPEN_JWT_SIGNING_KEY" => Some(OPEN_KEY.into()),
        _ => None,
    })
    .unwrap();
    app.router = build_router(AppState::new(config, Some(app.database.clone())));
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap().to_owned();
    let(code,created)=app.send("POST","/api/v1/applications",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Open auth","code":"open-auth","allowedScopes":["open.profile.read","open.profile.write","open.users.read","open.users.write"]}),Some(&admin)).await;
    assert_eq!(code, StatusCode::CREATED);
    let id = created["data"]["application"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let secret = created["data"]["secret"].as_str().unwrap().to_owned();
    assert_eq!(app.send("POST",&format!("/api/v1/applications/{id}/users"),json!({"username":"player.one","displayName":"Player","password":PASSWORD,"scopes":["open.profile.read","open.profile.write"]}),Some(&admin)).await.0,StatusCode::CREATED);
    (app, admin, id, secret)
}
async fn login(app: &TestApp, id: &str) -> Value {
    let (code, value) = app
        .send(
            "POST",
            "/open/v1/auth/login",
            json!({"applicationId":id,"username":"PLAYER.ONE","password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{value}");
    value["data"].clone()
}
#[tokio::test]
async fn open_users_have_independent_identity_access_and_rotating_refresh_sessions() {
    let (app, admin, id, secret) = fixture().await;
    let user = login(&app, &id).await;
    let access = user["accessToken"].as_str().unwrap();
    assert_eq!(user["expiresIn"], 900);
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(&admin))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (code, service) = app
        .send(
            "POST",
            "/open/v1/auth/application",
            json!({"applicationId":id,"secret":secret}),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    let service = service["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(service))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (code, next) = app
        .send(
            "POST",
            "/open/v1/auth/refresh",
            json!({"applicationId":id,"refreshToken":user["refreshToken"]}),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_ne!(next["data"]["refreshToken"], user["refreshToken"]);
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/auth/refresh",
            json!({"applicationId":id,"refreshToken":user["refreshToken"]}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "GET",
            "/open/v1/me",
            Value::Null,
            Some(next["data"]["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send("GET", "/api/v1/auth/me", Value::Null, Some(&admin))
            .await
            .0,
        StatusCode::OK
    );
    app.cleanup().await;
}
#[tokio::test]
async fn profile_writes_scope_changes_and_real_password_reset_revoke_open_credentials() {
    let (app, admin, id, _) = fixture().await;
    let user = login(&app, &id).await;
    let access = user["accessToken"].as_str().unwrap();
    let (code, profile) = app
        .send(
            "PATCH",
            "/open/v1/me",
            json!({"version":1,"displayName":"Updated"}),
            Some(access),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(profile["data"]["displayName"], "Updated");
    let uid = profile["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            "/open/v1/me",
            json!({"version":2,"status":"active"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}/users/{uid}"),
            json!({"version":2,"scopes":["open.profile.read"]}),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            "/open/v1/me",
            json!({"version":3,"displayName":"Denied"}),
            Some(access)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/applications/{id}/users/{uid}/password"),
            json!({"version":3,"password":"ChangedPlayerPassword-2026"}),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/auth/refresh",
            json!({"applicationId":id,"refreshToken":user["refreshToken"]}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}
#[tokio::test]
async fn open_keys_are_required_and_expired_sessions_or_cross_application_refresh_do_not_authenticate()
 {
    let (app, _admin, id, _) = fixture().await;
    let user = login(&app, &id).await;
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/auth/refresh",
            json!({"applicationId":ObjectId::new().to_hex(),"refreshToken":user["refreshToken"]}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.database
        .collection::<Document>("open_sessions")
        .update_many(
            doc! {},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "GET",
            "/open/v1/me",
            Value::Null,
            Some(user["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let config = Config::from_lookup(|key| {
        if key == "TRI_JWT_SIGNING_KEY" {
            Some(support::KEY.into())
        } else {
            None
        }
    })
    .unwrap();
    let mut unconfigured = app;
    unconfigured.router = build_router(AppState::new(config, Some(unconfigured.database.clone())));
    assert_eq!(
        unconfigured
            .send(
                "POST",
                "/open/v1/auth/login",
                json!({"applicationId":id,"username":"player.one","password":PASSWORD}),
                None
            )
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    unconfigured.cleanup().await;
}

#[tokio::test]
async fn real_service_rotation_and_application_disable_revoke_correct_principal_kinds() {
    let (app, admin, id, secret) = fixture().await;
    let user = login(&app, &id).await;
    let (code, service) = app
        .send(
            "POST",
            "/open/v1/auth/application",
            json!({"applicationId":id,"secret":secret}),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    let access = service["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/applications/{id}/secret"),
            json!({"version":1,"requestId":uuid::Uuid::new_v4().to_string()}),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "GET",
            "/open/v1/me",
            Value::Null,
            Some(user["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/auth/application",
            json!({"applicationId":id,"secret":secret}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.send(
        "PATCH",
        &format!("/api/v1/applications/{id}"),
        json!({"version":2,"enabled":false}),
        Some(&admin),
    )
    .await;
    assert_eq!(
        app.send(
            "GET",
            "/open/v1/me",
            Value::Null,
            Some(user["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.send(
        "PATCH",
        &format!("/api/v1/applications/{id}"),
        json!({"version":3,"enabled":true}),
        Some(&admin),
    )
    .await;
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/auth/refresh",
            json!({"applicationId":id,"refreshToken":user["refreshToken"]}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}
#[tokio::test]
async fn concurrent_refresh_reuse_revokes_the_session_and_refresh_never_expands_initial_grants() {
    let (app, _admin, id, _) = fixture().await;
    let(code,user)=app.send("POST","/open/v1/auth/login",json!({"applicationId":id,"username":"player.one","password":PASSWORD,"scopes":["open.profile.read"]}),None).await;
    assert_eq!(code, StatusCode::OK);
    let user = &user["data"];
    let body = json!({"applicationId":id,"refreshToken":user["refreshToken"]});
    let (one, two) = tokio::join!(
        app.send("POST", "/open/v1/auth/refresh", body.clone(), None),
        app.send("POST", "/open/v1/auth/refresh", body, None)
    );
    assert!([one.0, two.0].contains(&StatusCode::OK));
    assert!([one.0, two.0].contains(&StatusCode::UNAUTHORIZED));
    let success = if one.0 == StatusCode::OK {
        one.1
    } else {
        two.1
    };
    assert_eq!(success["data"]["scopes"], json!(["open.profile.read"]));
    assert_eq!(
        app.send(
            "GET",
            "/open/v1/me",
            Value::Null,
            Some(success["data"]["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}
#[tokio::test]
async fn application_and_user_calls_share_atomic_budgets_and_logs_have_no_admin_actor() {
    let (app, admin, id, secret) = fixture().await;
    let user = login(&app, &id).await;
    let (_, application) = app
        .send(
            "POST",
            "/open/v1/auth/application",
            json!({"applicationId":id,"secret":secret}),
            None,
        )
        .await;
    let service = application["data"]["accessToken"].as_str().unwrap();
    app.send(
        "PATCH",
        &format!("/api/v1/applications/{id}"),
        json!({"version":1,"minuteLimit":2,"dailyQuota":2}),
        Some(&admin),
    )
    .await;
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(service))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "GET",
            "/open/v1/me",
            Value::Null,
            Some(user["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            "/open/v1/me",
            Value::Null,
            Some(user["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let logs = app.database.collection::<Document>("application_call_logs");
    assert_eq!(logs.count_documents(doc! {}).await.unwrap(), 3);
    assert_eq!(
        logs.count_documents(doc! {"actorId":{"$exists":true}})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        logs.count_documents(doc! {"ownerId":{"$exists":true}})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        logs.count_documents(doc! {"state":"finished"})
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/auth/logout",
            json!({"applicationId":id,"refreshToken":user["refreshToken"]}),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/auth/application/logout",
            json!({}),
            Some(service)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    app.cleanup().await;
}

#[test]
fn open_signing_configuration_is_independent_redacted_and_rejects_short_keys() {
    assert!(
        Config::from_lookup(|key| match key {
            "TRI_JWT_SIGNING_KEY" | "TRI_OPEN_JWT_SIGNING_KEY" => Some(support::KEY.into()),
            _ => None,
        })
        .is_err()
    );
    assert!(
        Config::from_lookup(|key| match key {
            "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
            "TRI_OPEN_JWT_SIGNING_KEY" => Some("short".into()),
            _ => None,
        })
        .is_err()
    );
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_OPEN_JWT_SIGNING_KEY" => Some(OPEN_KEY.into()),
        _ => None,
    })
    .unwrap();
    let debug = format!("{config:?}");
    assert!(!debug.contains(OPEN_KEY));
    assert!(!debug.contains(support::KEY));
}

#[tokio::test]
async fn open_requests_use_socket_cidrs_and_reject_forged_forwarded_addresses() {
    use axum::{body::Body, extract::ConnectInfo, http::Request};
    use tower::ServiceExt;
    let (app, admin, id, _) = fixture().await;
    let user = login(&app, &id).await;
    let access = user["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}"),
            json!({"version":1,"allowedIps":["192.0.2.0/24"]}),
            Some(&admin)
        )
        .await
        .0,
        StatusCode::OK
    );
    for (address, expected) in [
        ("203.0.113.2:9000", StatusCode::FORBIDDEN),
        ("192.0.2.9:9000", StatusCode::OK),
    ] {
        let request = Request::builder()
            .uri("/open/v1/me")
            .header("authorization", format!("Bearer {access}"))
            .header("x-forwarded-for", "192.0.2.9")
            .extension(ConnectInfo(
                address.parse::<std::net::SocketAddr>().unwrap(),
            ))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.router.clone().oneshot(request).await.unwrap().status(),
            expected
        );
    }
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn invalid_refresh_does_not_disclose_restricted_source_policy_before_proof() {
    let (app, admin, id, _) = fixture().await;
    let user = login(&app, &id).await;
    let sid = admin_api::modules::open_access::credential::parse(
        "tri_orf1",
        user["refreshToken"].as_str().unwrap(),
    )
    .unwrap();
    let (wrong, _) = admin_api::modules::open_access::credential::issue("tri_orf1", sid).unwrap();
    app.send(
        "PATCH",
        &format!("/api/v1/applications/{id}"),
        json!({"version":1,"allowedIps":["192.0.2.0/24"]}),
        Some(&admin),
    )
    .await;
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/auth/refresh",
            json!({"applicationId":id,"refreshToken":wrong}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}
