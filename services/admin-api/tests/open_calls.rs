mod support;
use admin_api::{build_router, config::Config, state::AppState};
use axum::http::StatusCode;
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
async fn fixture() -> (TestApp, String, String, String) {
    let mut app = TestApp::new().await;
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_OPEN_JWT_SIGNING_KEY" => {
            Some("open-calls-fixture-key-independent-from-admin-2026".into())
        }
        _ => None,
    })
    .unwrap();
    app.router = build_router(AppState::new(config, Some(app.database.clone())));
    let auth = app.login().await;
    let admin = auth["accessToken"].as_str().unwrap().to_owned();
    let (id, secret) = application(&app, &admin, "calls-app").await;
    (app, admin, id, secret)
}
async fn application(app: &TestApp, admin: &str, code: &str) -> (String, String) {
    let(status,value)=app.send("POST","/api/v1/applications",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":code,"code":code,"allowedScopes":["open.profile.read","open.profile.write","open.users.read","open.users.write"]}),Some(admin)).await;
    assert_eq!(status, StatusCode::CREATED);
    (
        value["data"]["application"]["id"].as_str().unwrap().into(),
        value["data"]["secret"].as_str().unwrap().into(),
    )
}
async fn service(app: &TestApp, id: &str, secret: &str, scopes: Option<Value>) -> String {
    let mut body = json!({"applicationId":id,"secret":secret});
    if let Some(scopes) = scopes {
        body["scopes"] = scopes;
    }
    let (code, value) = app
        .send("POST", "/open/v1/auth/application", body, None)
        .await;
    assert_eq!(code, StatusCode::OK);
    value["data"]["accessToken"].as_str().unwrap().into()
}
fn create(name: &str) -> Value {
    json!({"username":name,"displayName":"Provisioned","password":PASSWORD,"scopes":["open.profile.read","open.profile.write"]})
}
#[tokio::test]
async fn application_principals_manage_their_users_and_password_deletion_revoke_real_user_tokens() {
    let (app, admin, id, secret) = fixture().await;
    let token = service(&app, &id, &secret, None).await;
    let (code, created) = app
        .send(
            "POST",
            "/open/v1/users",
            create("provisioned.user"),
            Some(&token),
        )
        .await;
    assert_eq!(code, StatusCode::CREATED, "{created}");
    let uid = created["data"]["id"].as_str().unwrap();
    assert_eq!(created["data"]["applicationId"], id);
    assert!(!created.to_string().contains("passwordHash"));
    assert_eq!(
        app.send(
            "GET",
            &format!("/open/v1/users/{uid}"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (code, _) = app
        .send(
            "PATCH",
            &format!("/open/v1/users/{uid}"),
            json!({"version":1,"displayName":"Service updated"}),
            Some(&token),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    let (code, user) = app
        .send(
            "POST",
            "/open/v1/auth/login",
            json!({"applicationId":id,"username":"provisioned.user","password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    let user_token = user["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/open/v1/users", Value::Null, Some(user_token))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/open/v1/users/{uid}/password"),
            json!({"version":2,"password":"ServiceResetPassword-2026"}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(user_token))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let(_,user)=app.send("POST","/open/v1/auth/login",json!({"applicationId":id,"username":"provisioned.user","password":"ServiceResetPassword-2026"}),None).await;
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/open/v1/users/{uid}?version=3"),
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
            "/open/v1/me",
            Value::Null,
            Some(user["data"]["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (code, replacement) = app
        .send(
            "POST",
            "/open/v1/users",
            create("PROVISIONED.USER"),
            Some(&token),
        )
        .await;
    assert_eq!(code, StatusCode::CREATED);
    assert_ne!(replacement["data"]["id"], uid);
    assert_eq!(
        app.send(
            "GET",
            &format!("/open/v1/users/{uid}"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send("GET", "/open/v1/users", Value::Null, Some(&admin))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let(_,pat)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Wrong realm","scopes":["applicationUsers.read","applications.read"]}),Some(&admin)).await;
    assert_eq!(
        app.send(
            "GET",
            "/open/v1/users",
            Value::Null,
            Some(pat["data"]["secret"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let event = app
        .database
        .collection::<Document>("audit_events")
        .find_one(doc! {"action":"open.users.create"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.get_object_id("applicationId").unwrap().to_hex(), id);
    assert!(matches!(
        event.get("actorId"),
        Some(mongodb::bson::Bson::Null)
    ));
    assert_eq!(event.get_str("principalKind").unwrap(), "application");
    app.cleanup().await;
}
#[tokio::test]
async fn service_scope_delegation_and_foreign_ids_never_expand_application_authority() {
    let (app, admin, id, secret) = fixture().await;
    let full = service(&app, &id, &secret, None).await;
    let (other, other_secret) = application(&app, &admin, "other-calls-app").await;
    let foreign_token = service(&app, &other, &other_secret, None).await;
    let (code, target) = app
        .send(
            "POST",
            "/open/v1/users",
            create("foreign.user"),
            Some(&foreign_token),
        )
        .await;
    assert_eq!(code, StatusCode::CREATED);
    let uid = target["data"]["id"].as_str().unwrap();
    for (method, path, body) in [
        ("GET", format!("/open/v1/users/{uid}"), Value::Null),
        (
            "PATCH",
            format!("/open/v1/users/{uid}"),
            json!({"version":1,"displayName":"No"}),
        ),
        (
            "DELETE",
            format!("/open/v1/users/{uid}?version=1"),
            Value::Null,
        ),
        (
            "POST",
            format!("/open/v1/users/{uid}/password"),
            json!({"version":1,"password":PASSWORD}),
        ),
    ] {
        assert_eq!(
            app.send(method, &path, body, Some(&full)).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let read = service(&app, &id, &secret, Some(json!(["open.users.read"]))).await;
    assert_eq!(
        app.send("GET", "/open/v1/users", Value::Null, Some(&read))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("POST", "/open/v1/users", create("denied.user"), Some(&read))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let bounded = service(
        &app,
        &id,
        &secret,
        Some(json!(["open.users.write", "open.profile.read"])),
    )
    .await;
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/users",
            create("too-powerful.user"),
            Some(&bounded)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let mut low = create("bounded.user");
    low["scopes"] = json!(["open.profile.read"]);
    assert_eq!(
        app.send("POST", "/open/v1/users", low, Some(&bounded))
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(app.send("POST","/open/v1/users",json!({"applicationId":other,"username":"injected.user","displayName":"No","password":PASSWORD}),Some(&full)).await.0,StatusCode::UNPROCESSABLE_ENTITY);
    app.send(
        "PATCH",
        &format!("/api/v1/applications/{id}"),
        json!({"version":1,"allowedScopes":["open.users.read","open.profile.read"]}),
        Some(&admin),
    )
    .await;
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/users",
            create("revoked.user"),
            Some(&full)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}
#[tokio::test]
async fn application_call_paging_filters_actual_identity_and_denies_missing_log_permission() {
    let (app, admin, id, secret) = fixture().await;
    let token = service(&app, &id, &secret, None).await;
    for _ in 0..23 {
        assert_eq!(
            app.send("GET", "/open/v1/users", Value::Null, Some(&token))
                .await
                .0,
            StatusCode::OK
        );
    }
    let (other, other_secret) = application(&app, &admin, "log-isolated-app").await;
    let unrelated = service(&app, &other, &other_secret, None).await;
    for _ in 0..4 {
        app.send("GET", "/open/v1/users", Value::Null, Some(&unrelated))
            .await;
    }
    let(code,page)=app.send("GET",&format!("/api/v1/applications/{id}/calls?page=2&pageSize=20&principalKind=application&status=200"),Value::Null,Some(&admin)).await;
    assert_eq!(code, StatusCode::OK, "{page}");
    assert_eq!(page["meta"]["total"], 23);
    assert_eq!(page["data"].as_array().unwrap().len(), 3);
    assert!(
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["applicationId"] == id)
    );
    assert!(!page.to_string().contains(&token));
    assert!(!page.to_string().contains("ownerId"));
    for query in [
        "page=0",
        "status=999",
        "principalKind=administrator",
        "from=not-a-date",
    ] {
        assert_eq!(
            app.send(
                "GET",
                &format!("/api/v1/applications/{id}/calls?{query}"),
                Value::Null,
                Some(&admin)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let(_,role)=app.send("POST","/api/v1/roles",json!({"name":"No logs","slug":"no-app-logs","permissions":["applications.read"],"dataScope":"self"}),Some(&admin)).await;
    app.send("POST","/api/v1/users",json!({"username":"no-app-logs","displayName":"No logs","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(&admin)).await;
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"no-app-logs","password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}/calls"),
            Value::Null,
            Some(login["data"]["accessToken"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}/calls"),
            Value::Null,
            Some(&token)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.database
            .collection::<Document>("application_call_logs")
            .count_documents(
                doc! {"applicationId":ObjectId::parse_str(&id).unwrap(),"actorId":{"$exists":true}}
            )
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/applications/{id}?version=1"),
            Value::Null,
            Some(&admin)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (code, historical) = app
        .send(
            "GET",
            &format!("/api/v1/applications/{id}/calls"),
            Value::Null,
            Some(&admin),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(historical["meta"]["total"], 23);
    app.cleanup().await;
}

#[tokio::test]
async fn cached_application_proof_is_reauthorized_inside_user_mutation_transaction() {
    use admin_api::modules::{
        applications::{dto::CreateUser, user_authority::Authority, users},
        open_access::{context::OpenContext, jwt, sessions::Record},
    };
    let (app, admin, id, secret) = fixture().await;
    let token = service(&app, &id, &secret, None).await;
    let config = Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_OPEN_JWT_SIGNING_KEY" => {
            Some("open-calls-fixture-key-independent-from-admin-2026".into())
        }
        _ => None,
    })
    .unwrap();
    let claims = jwt::decode(&config, &token).unwrap();
    let record = app
        .database
        .collection::<Record>("open_sessions")
        .find_one(doc! {"_id":ObjectId::parse_str(&claims.sid).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let context = OpenContext {
        claims,
        scopes: record.grant.clone(),
        session: record,
    };
    let before = app
        .database
        .collection::<Document>("open_sessions")
        .find_one(doc! {"_id":context.session.id})
        .await
        .unwrap()
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"open-access"}, doc! {"$set":{"version":3}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        before,
        app.database
            .collection::<Document>("open_sessions")
            .find_one(doc! {"_id":context.session.id})
            .await
            .unwrap()
            .unwrap()
    );
    let authority = Authority::application(&context).unwrap();
    app.send(
        "PATCH",
        &format!("/api/v1/applications/{id}"),
        json!({"version":1,"allowedScopes":["open.users.read","open.profile.read"]}),
        Some(&admin),
    )
    .await;
    let input: CreateUser = serde_json::from_value(create("stale-proof.user")).unwrap();
    let result = users::create(
        &app.database,
        authority,
        ObjectId::parse_str(&id).unwrap(),
        input,
    )
    .await;
    assert!(matches!(
        result,
        Err(admin_api::common::error::AppError::Forbidden)
    ));
    assert_eq!(
        app.database
            .collection::<Document>("application_users")
            .count_documents(doc! {"usernameKey":"stale-proof.user"})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn application_audit_and_call_log_failures_do_not_commit_unaudited_user_or_password_changes()
{
    let (app, admin, id, secret) = fixture().await;
    let token = service(&app, &id, &secret, None).await;
    let (_, account) = app
        .send(
            "POST",
            "/open/v1/users",
            create("audited.user"),
            Some(&token),
        )
        .await;
    let uid = account["data"]["id"].as_str().unwrap();
    let (_, login) = app
        .send(
            "POST",
            "/open/v1/auth/login",
            json!({"applicationId":id,"username":"audited.user","password":PASSWORD}),
            None,
        )
        .await;
    let access = login["data"]["accessToken"].as_str().unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$nin":["open.users.resetPassword","open.users.create"]}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/open/v1/users/{uid}/password"),
            json!({"version":1,"password":"MustRollbackPassword-2026"}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.send("GET", "/open/v1/me", Value::Null, Some(access))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/users",
            create("no-audit.user"),
            Some(&token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("application_users")
            .count_documents(doc! {"usernameKey":"no-audit.user"})
            .await
            .unwrap(),
        0
    );
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    app.database.run_command(doc!{"collMod":"application_call_logs","validator":{"principalKind":{"$exists":false}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/users",
            create("no-call-log.user"),
            Some(&token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("application_users")
            .count_documents(doc! {"usernameKey":"no-call-log.user"})
            .await
            .unwrap(),
        0
    );
    app.database.run_command(doc!{"collMod":"application_call_logs","validator":{"state":{"$ne":"finished"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/open/v1/users",
            create("confirmed-user.pending-log"),
            Some(&token)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (code, page) = app
        .send(
            "GET",
            &format!("/api/v1/applications/{id}/calls?state=started"),
            Value::Null,
            Some(&admin),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(page["meta"]["total"], 1);
    assert!(page["data"][0]["status"].is_null());
    app.cleanup().await;
}
