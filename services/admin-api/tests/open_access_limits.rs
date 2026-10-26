mod support;
use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use support::TestApp;
use tower::ServiceExt;
async fn issue(app: &TestApp, session: &str, policy: Value) -> String {
    let mut input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Budget","scopes":["parameters.read","parameters.write"]});
    for (key, value) in policy.as_object().unwrap() {
        input[key] = value.clone();
    }
    let (code, result) = app
        .send("POST", "/api/v1/api-tokens", input, Some(session))
        .await;
    assert_eq!(code, StatusCode::CREATED, "{result}");
    result["data"]["secret"].as_str().unwrap().into()
}
async fn peer(
    app: &TestApp,
    token: &str,
    address: Option<&str>,
    forwarded: Option<&str>,
) -> StatusCode {
    let mut request = Request::builder()
        .uri("/api/v1/parameters")
        .header("authorization", format!("Bearer {token}"));
    if let Some(address) = address {
        request = request.extension(ConnectInfo(
            address.parse::<std::net::SocketAddr>().unwrap(),
        ));
    }
    if let Some(forwarded) = forwarded {
        request = request.header("x-forwarded-for", forwarded);
    }
    app.router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}
#[tokio::test]
async fn token_limits_are_atomic_and_failures_do_not_charge_a_guessed_token() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let token = issue(&app, session, json!({"minuteLimit":3,"dailyQuota":2})).await;
    let results = futures_util::future::join_all(
        (0..12).map(|_| app.send("GET", "/api/v1/parameters", Value::Null, Some(&token))),
    )
    .await;
    assert_eq!(
        results.iter().filter(|row| row.0 == StatusCode::OK).count(),
        2
    );
    assert_eq!(
        results
            .iter()
            .filter(|row| row.0 == StatusCode::TOO_MANY_REQUESTS)
            .count(),
        10
    );
    let rows = app
        .database
        .collection::<Document>("api_usage_buckets")
        .find(doc! {})
        .await
        .unwrap();
    use futures_util::TryStreamExt;
    let counts = rows
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get_i64("count").unwrap())
        .collect::<Vec<_>>();
    assert!(counts.iter().all(|value| *value <= 2));
    assert_eq!(counts.iter().sum::<i64>(), 4);
    let mut invalid = token.clone().into_bytes();
    let last = invalid.len() - 2;
    invalid[last] = if invalid[last] == b'A' { b'B' } else { b'A' };
    let invalid = String::from_utf8(invalid).unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/parameters", Value::Null, Some(&invalid))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.database
            .collection::<Document>("api_call_logs")
            .count_documents(doc! {})
            .await
            .unwrap(),
        12
    );
    let request = Request::builder()
        .uri("/api/v1/parameters")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        response.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    let _ = to_bytes(response.into_body(), 65536).await.unwrap();
    let (code, page) = app
        .send(
            "GET",
            "/api/v1/api-tokens/calls?pageSize=100",
            Value::Null,
            Some(session),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(page["meta"]["total"], 13);
    assert!(
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["state"] == "finished")
    );
    assert!(!page.to_string().contains(&token));
    assert!(!page.to_string().contains("secretHash"));
    app.cleanup().await;
}
#[tokio::test]
async fn ip_rules_use_socket_addresses_and_unknown_sources_or_forged_headers_do_not_pass() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let token = issue(
        &app,
        session,
        json!({"allowedIps":["192.0.2.7/24","2001:db8::/32"]}),
    )
    .await;
    assert_eq!(
        peer(&app, &token, Some("192.0.2.42:9000"), None).await,
        StatusCode::OK
    );
    assert_eq!(
        peer(&app, &token, Some("[2001:db8::7]:9000"), None).await,
        StatusCode::OK
    );
    assert_eq!(
        peer(&app, &token, Some("[::ffff:192.0.2.42]:9000"), None).await,
        StatusCode::OK
    );
    assert_eq!(
        peer(&app, &token, Some("203.0.113.2:9000"), Some("192.0.2.42")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        peer(&app, &token, None, Some("192.0.2.42")).await,
        StatusCode::FORBIDDEN
    );
    let (_, page) = app
        .send(
            "GET",
            "/api/v1/api-tokens/calls",
            Value::Null,
            Some(session),
        )
        .await;
    assert_eq!(page["meta"]["total"], 5);
    assert_eq!(
        app.database
            .collection::<Document>("api_call_logs")
            .count_documents(doc! {"status":403})
            .await
            .unwrap(),
        2
    );
    app.cleanup().await;
}
#[tokio::test]
async fn call_log_failure_prevents_business_changes_and_scope_rejections_are_recorded() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let token = issue(&app, session, json!({})).await;
    assert_eq!(
        app.send("GET", "/api/v1/roles", Value::Null, Some(&token))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.database
            .collection::<Document>("api_call_logs")
            .count_documents(doc! {"status":403,"state":"finished"})
            .await
            .unwrap(),
        1
    );
    app.database.run_command(doc!{"collMod":"api_call_logs","validator":{"ownerId":{"$exists":false}},"validationLevel":"strict"}).await.unwrap();
    let input =
        json!({"name":"Must not write","key":"blocked-by-log","valueType":"boolean","value":true});
    assert_eq!(
        app.send("POST", "/api/v1/parameters", input, Some(&token))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {"key":"blocked-by-log"})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn failed_completion_logging_keeps_a_pending_receipt_without_reversing_successful_business() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let token = issue(&app, session, json!({})).await;
    app.database.run_command(doc!{"collMod":"api_call_logs","validator":{"state":{"$ne":"finished"}},"validationLevel":"strict"}).await.unwrap();
    let (code,_)=app.send("POST","/api/v1/parameters",json!({"name":"Stored","key":"completion-log-failed","valueType":"boolean","value":true}),Some(&token)).await;
    assert_eq!(code, StatusCode::CREATED);
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {"key":"completion-log-failed"})
            .await
            .unwrap(),
        1
    );
    let (_, page) = app
        .send(
            "GET",
            "/api/v1/api-tokens/calls",
            Value::Null,
            Some(session),
        )
        .await;
    assert_eq!(page["meta"]["total"], 1);
    assert_eq!(page["data"][0]["state"], "started");
    assert!(page["data"][0]["status"].is_null());
    app.cleanup().await;
}

#[tokio::test]
async fn multiple_authentication_extractors_in_one_request_charge_one_call() {
    use admin_api::{config::Config, modules::auth::AuthContext, state::AppState};
    let mut app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let token = issue(&app, session, json!({"minuteLimit":1,"dailyQuota":1})).await;
    let config = Config::from_lookup(|name| {
        if name == "TRI_JWT_SIGNING_KEY" {
            Some(support::KEY.into())
        } else {
            None
        }
    })
    .unwrap();
    let state = AppState::new(config, Some(app.database.clone()));
    app.router = axum::Router::new()
        .route(
            "/api/v1/parameters",
            axum::routing::get(|one: AuthContext, two: AuthContext| async move {
                assert_eq!(one.user.id, two.user.id);
                StatusCode::NO_CONTENT
            }),
        )
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state,
            admin_api::common::observability::observe,
        ));
    assert_eq!(peer(&app, &token, None, None).await, StatusCode::NO_CONTENT);
    assert_eq!(
        app.database
            .collection::<Document>("api_call_logs")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn old_call_logs_are_hidden_and_bounded_cleanup_does_not_depend_on_ttl_deletion() {
    use mongodb::bson::{DateTime, oid::ObjectId};
    let app = TestApp::new().await;
    let auth = app.login().await;
    let session = auth["accessToken"].as_str().unwrap();
    let owner = ObjectId::parse_str(auth["user"]["id"].as_str().unwrap()).unwrap();
    let old = DateTime::from_millis(DateTime::now().timestamp_millis() - 31 * 86400000);
    let future = DateTime::from_millis(DateTime::now().timestamp_millis() + 86400000);
    let rows = (0..256)
        .map(|_| doc! {"_id":ObjectId::new(),"ownerId":owner,"createdAt":old,"expireAt":future})
        .collect::<Vec<_>>();
    app.database
        .collection::<Document>("api_call_logs")
        .insert_many(rows)
        .await
        .unwrap();
    let (_, page) = app
        .send(
            "GET",
            "/api/v1/api-tokens/calls",
            Value::Null,
            Some(session),
        )
        .await;
    assert_eq!(page["meta"]["total"], 0);
    for count in [100, 100, 56, 0] {
        assert_eq!(
            admin_api::modules::open_access::usage::maintenance_once(&app.database)
                .await
                .unwrap()
                .calls,
            count
        );
    }
    app.cleanup().await;
}
