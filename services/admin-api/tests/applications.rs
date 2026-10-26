mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};
fn input(code: &str, request: &str) -> Value {
    json!({"requestId":request,"name":"Game service","code":code,"allowedScopes":["open.profile.read","open.profile.write","open.users.read","open.users.write"]})
}
async fn create(app: &TestApp, token: &str, code: &str) -> Value {
    let (status, value) = app
        .send(
            "POST",
            "/api/v1/applications",
            input(code, &uuid::Uuid::new_v4().to_string()),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
async fn user(app: &TestApp, token: &str, application: &str, name: &str) -> Value {
    let(status,value)=app.send("POST",&format!("/api/v1/applications/{application}/users"),json!({"username":name,"displayName":"Game user","password":PASSWORD,"scopes":["open.profile.read","open.profile.write"]}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
#[tokio::test]
async fn applications_issue_one_secret_with_actor_bound_recovery_and_never_reuse_codes() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let body = input("game-one", &request);
    let (status, issued) = app
        .send("POST", "/api/v1/applications", body.clone(), Some(token))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let id = issued["data"]["application"]["id"].as_str().unwrap();
    let secret = issued["data"]["secret"].as_str().unwrap();
    assert!(secret.starts_with("tri_app1."));
    let raw = app
        .database
        .collection::<Document>("applications")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_str("secretHash").unwrap().len(), 64);
    assert!(!format!("{raw:?}").contains(secret));
    let (status, replay) = app
        .send("POST", "/api/v1/applications", body.clone(), Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["application"]["id"], id);
    assert!(replay["data"]["secret"].is_null());
    let (_, found) = app
        .send(
            "GET",
            &format!("/api/v1/applications/by-request/{request}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(found["data"]["id"], id);
    assert!(!found.to_string().contains("secretHash"));
    let mut changed = body.clone();
    changed["name"] = json!("different");
    assert_eq!(
        app.send("POST", "/api/v1/applications", changed, Some(token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/applications/{id}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/applications",
            input("game-one", &uuid::Uuid::new_v4().to_string()),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, replay) = app
        .send("POST", "/api/v1/applications", body, Some(token))
        .await;
    assert_eq!(replay["data"]["application"]["id"], id);
    assert!(replay["data"]["application"]["deletedAt"].is_string());
    assert!(replay["data"]["secret"].is_null());
    app.cleanup().await;
}
#[tokio::test]
async fn concurrent_creation_and_rotation_have_one_identity_and_one_disclosure() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let body = input("game-concurrent", &request);
    let (one, two) = tokio::join!(
        app.send("POST", "/api/v1/applications", body.clone(), Some(token)),
        app.send("POST", "/api/v1/applications", body, Some(token))
    );
    assert!([one.0, two.0].contains(&StatusCode::CREATED));
    assert!([one.0, two.0].contains(&StatusCode::OK));
    assert_eq!(
        [&one.1, &two.1]
            .iter()
            .filter(|v| v["data"]["secret"].is_string())
            .count(),
        1
    );
    let issued = if one.1["data"]["secret"].is_string() {
        one.1
    } else {
        two.1
    };
    let id = issued["data"]["application"]["id"].as_str().unwrap();
    let old = issued["data"]["secret"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let path = format!("/api/v1/applications/{id}/secret");
    let body = json!({"requestId":request,"version":1});
    let (one, two) = tokio::join!(
        app.send("POST", &path, body.clone(), Some(token)),
        app.send("POST", &path, body.clone(), Some(token))
    );
    assert_eq!(one.0, StatusCode::OK);
    assert_eq!(two.0, StatusCode::OK);
    assert_eq!(
        [&one.1, &two.1]
            .iter()
            .filter(|v| v["data"]["secret"].is_string())
            .count(),
        1
    );
    let rotation = if one.1["data"]["secret"].is_string() {
        one.1
    } else {
        two.1
    };
    let new = rotation["data"]["secret"].as_str().unwrap();
    let raw = app
        .database
        .collection::<Document>("applications")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let hash = raw.get_str("secretHash").unwrap();
    assert!(admin_api::modules::open_access::credential::verify(
        new, hash
    ));
    assert!(!admin_api::modules::open_access::credential::verify(
        old, hash
    ));
    assert_eq!(raw.get_i64("credentialVersion").unwrap(), 2);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}"),
            json!({"version":2,"name":"Renamed"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, replay) = app.send("POST", &path, body, Some(token)).await;
    assert!(replay["data"]["secret"].is_null());
    assert_eq!(replay["data"]["receipt"]["credentialVersion"], 2);
    assert_eq!(replay["data"]["application"]["version"], 3);
    let (_, receipt) = app
        .send(
            "GET",
            &format!("/api/v1/applications/{id}/actions/{request}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(receipt["data"]["credentialVersion"], 2);
    assert!(!receipt.to_string().contains(new));
    assert_eq!(
        app.send(
            "POST",
            &path,
            json!({"requestId":request,"version":3}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"applications.rotateSecret"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn app_users_are_scoped_unique_private_and_password_resets_revoke_only_their_sessions() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let a = create(&app, token, "game-a").await;
    let b = create(&app, token, "game-b").await;
    let a = a["application"]["id"].as_str().unwrap();
    let b = b["application"]["id"].as_str().unwrap();
    let one = user(&app, token, a, "shared.user").await;
    let two = user(&app, token, b, "SHARED.USER").await;
    let id = one["id"].as_str().unwrap();
    assert_ne!(one["id"], two["id"]);
    assert_eq!(app.send("POST",&format!("/api/v1/applications/{a}/users"),json!({"username":"SHARED.USER","displayName":"duplicate","password":PASSWORD,"scopes":["open.profile.read"]}),Some(token)).await.0,StatusCode::CONFLICT);
    for method in ["GET", "PATCH", "DELETE"] {
        let path = if method == "DELETE" {
            format!("/api/v1/applications/{b}/users/{id}?version=1")
        } else {
            format!("/api/v1/applications/{b}/users/{id}")
        };
        assert_eq!(
            app.send(
                method,
                &path,
                if method == "PATCH" {
                    json!({"version":1,"displayName":"forbidden"})
                } else {
                    Value::Null
                },
                Some(token)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let first = ObjectId::new();
    let other = ObjectId::new();
    let application = ObjectId::new();
    let now = DateTime::now();
    for (sid, app_id, user_id, kind) in [
        (first, a, Some(ObjectId::parse_str(id).unwrap()), "user"),
        (
            other,
            b,
            Some(ObjectId::parse_str(two["id"].as_str().unwrap()).unwrap()),
            "user",
        ),
        (application, a, None, "application"),
    ] {
        app.database.collection::<Document>("open_sessions").insert_one(doc!{"_id":sid,"applicationId":ObjectId::parse_str(app_id).unwrap(),"userId":user_id,"principalKind":kind,"status":"active","revokedAt":null,"createdAt":now,"expiresAt":DateTime::from_millis(now.timestamp_millis()+86400000)}).await.unwrap();
    }
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/applications/{a}/users/{id}/password"),
            json!({"version":1,"password":"NewGamePassword-2026"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let raw = app
        .database
        .collection::<Document>("application_users")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let hash = raw.get_str("passwordHash").unwrap().to_owned();
    assert!(hash.starts_with("$argon2id$"));
    assert!(
        admin_api::common::password::verify("NewGamePassword-2026".into(), hash.clone())
            .await
            .unwrap()
    );
    assert!(
        !admin_api::common::password::verify(PASSWORD.into(), hash)
            .await
            .unwrap()
    );
    let rows = app.database.collection::<Document>("open_sessions");
    assert_eq!(
        rows.find_one(doc! {"_id":first})
            .await
            .unwrap()
            .unwrap()
            .get_str("status")
            .unwrap(),
        "revoked"
    );
    for sid in [other, application] {
        assert_eq!(
            rows.find_one(doc! {"_id":sid})
                .await
                .unwrap()
                .unwrap()
                .get_str("status")
                .unwrap(),
            "active"
        );
    }
    let (_, page) = app
        .send(
            "GET",
            &format!("/api/v1/applications/{a}/users"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(page["meta"]["total"], 1);
    assert!(!page.to_string().contains("passwordHash"));
    assert!(!page.to_string().contains(PASSWORD));
    app.cleanup().await;
}

#[tokio::test]
async fn audit_failures_roll_back_rotation_password_and_session_changes() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let issued = create(&app, token, "audit-game").await;
    let id = issued["application"]["id"].as_str().unwrap();
    let object = ObjectId::parse_str(id).unwrap();
    let account = user(&app, token, id, "audit.user").await;
    let user_id = ObjectId::parse_str(account["id"].as_str().unwrap()).unwrap();
    let sid = ObjectId::new();
    app.database.collection::<Document>("open_sessions").insert_one(doc!{"_id":sid,"applicationId":object,"principalKind":"user","userId":user_id,"status":"active","revokedAt":null}).await.unwrap();
    let before = app
        .database
        .collection::<Document>("applications")
        .find_one(doc! {"_id":object})
        .await
        .unwrap()
        .unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$nin":["applications.rotateSecret","applicationUsers.resetPassword","applicationUsers.create"]}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/applications/{id}/secret"),
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("applications")
            .find_one(doc! {"_id":object})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        app.database
            .collection::<Document>("application_actions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/applications/{id}/users/{user_id}/password"),
            json!({"version":1,"password":"AuditMustRollback-2026"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let raw = app
        .database
        .collection::<Document>("application_users")
        .find_one(doc! {"_id":user_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_i64("version").unwrap(), 1);
    assert!(
        admin_api::common::password::verify(
            PASSWORD.into(),
            raw.get_str("passwordHash").unwrap().into()
        )
        .await
        .unwrap()
    );
    assert_eq!(
        app.database
            .collection::<Document>("open_sessions")
            .find_one(doc! {"_id":sid})
            .await
            .unwrap()
            .unwrap()
            .get_str("status")
            .unwrap(),
        "active"
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/applications/{id}/users"),
            json!({"username":"rollback-user","displayName":"No insert","password":PASSWORD}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("application_users")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn disabling_and_deleting_parents_or_users_never_revives_old_sessions_or_rebinds_names() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let created = create(&app, token, "lifecycle-game").await;
    let id = created["application"]["id"].as_str().unwrap();
    let object = ObjectId::parse_str(id).unwrap();
    let account = user(&app, token, id, "reusable.user").await;
    let uid = ObjectId::parse_str(account["id"].as_str().unwrap()).unwrap();
    let app_sid = ObjectId::new();
    let user_sid = ObjectId::new();
    for (sid, kind, subject) in [
        (app_sid, "application", None),
        (user_sid, "user", Some(uid)),
    ] {
        app.database.collection::<Document>("open_sessions").insert_one(doc!{"_id":sid,"applicationId":object,"principalKind":kind,"userId":subject,"status":"active","revokedAt":null}).await.unwrap();
    }
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}/users/{uid}"),
            json!({"version":1,"status":"disabled"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.database
            .collection::<Document>("open_sessions")
            .find_one(doc! {"_id":user_sid})
            .await
            .unwrap()
            .unwrap()
            .get_str("status")
            .unwrap(),
        "revoked"
    );
    assert_eq!(
        app.database
            .collection::<Document>("open_sessions")
            .find_one(doc! {"_id":app_sid})
            .await
            .unwrap()
            .unwrap()
            .get_str("status")
            .unwrap(),
        "active"
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/applications/{id}/users/{uid}?version=2"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let replacement = user(&app, token, id, "REUSABLE.USER").await;
    assert_ne!(replacement["id"], account["id"]);
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}/users/{uid}"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}"),
            json!({"version":1,"enabled":false}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}"),
            json!({"version":2,"enabled":true}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.database
            .collection::<Document>("open_sessions")
            .count_documents(doc! {"applicationId":object,"status":"active"})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/applications/{id}?version=3"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}/users"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.database
            .collection::<Document>("application_users")
            .count_documents(doc! {"applicationId":object})
            .await
            .unwrap(),
        2
    );
    app.cleanup().await;
}

async fn login_role(app: &TestApp, owner: &str, slug: &str, permissions: Vec<&str>) -> String {
    let (status, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":slug,"slug":slug,"permissions":permissions,"dataScope":"self"}),
            Some(owner),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let(status,_)=app.send("POST","/api/v1/users",json!({"username":slug,"displayName":slug,"password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":slug,"password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    login["data"]["accessToken"].as_str().unwrap().into()
}
#[tokio::test]
async fn configuration_read_or_write_does_not_grant_user_data_or_more_powerful_credentials() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let item = create(&app, owner, "permission-game").await;
    let id = item["application"]["id"].as_str().unwrap();
    user(&app, owner, id, "private.user").await;
    let reader = login_role(&app, owner, "app-reader", vec!["applications.read"]).await;
    assert_eq!(
        app.send("GET", "/api/v1/applications", Value::Null, Some(&reader))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}/users"),
            Value::Null,
            Some(&reader)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let config = login_role(
        &app,
        owner,
        "app-config",
        vec!["applications.read", "applications.write"],
    )
    .await;
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/applications",
            input("must-not-grant", &uuid::Uuid::new_v4().to_string()),
            Some(&config)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}"),
            json!({"version":1,"name":"Metadata only"}),
            Some(&config)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/applications/{id}/secret"),
            json!({"version":2,"requestId":uuid::Uuid::new_v4().to_string()}),
            Some(&config)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}"),
            json!({"version":2,"allowedScopes":["open.users.read"]}),
            Some(&config)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let users_reader = login_role(
        &app,
        owner,
        "app-users-reader",
        vec!["applications.read", "applicationUsers.read"],
    )
    .await;
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}/users"),
            Value::Null,
            Some(&users_reader)
        )
        .await
        .0,
        StatusCode::OK
    );
    let(_,pat)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"No credential mint","scopes":["applications.read","applications.write","applicationUsers.read","applicationUsers.write"]}),Some(owner)).await;
    let pat = pat["data"]["secret"].as_str().unwrap();
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}/users"),
            Value::Null,
            Some(pat)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/applications",
            input("must-not-mint", &uuid::Uuid::new_v4().to_string()),
            Some(pat)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/applications/{id}/users"),
            json!({"username":"must-not-mint","displayName":"Denied","password":PASSWORD}),
            Some(pat)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn own_v1_upgrade_preserves_tokens_and_users_scope_version_and_password_inputs_are_strict() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let(_,issued)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Preserved","scopes":["parameters.read"]}),Some(token)).await;
    let pat_id = ObjectId::parse_str(issued["data"]["token"]["id"].as_str().unwrap()).unwrap();
    let before = app
        .database
        .collection::<Document>("api_tokens")
        .find_one(doc! {"_id":pat_id})
        .await
        .unwrap()
        .unwrap();
    let menu = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"viewKey":"parameters","deletedAt":null})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("_id")
        .unwrap();
    app.database
        .collection::<Document>("menus")
        .update_one(
            doc! {"_id":menu},
            doc! {"$set":{"label":"Keep custom navigation","enabled":false}},
        )
        .await
        .unwrap();
    for name in [
        "applications",
        "application_actions",
        "application_users",
        "open_sessions",
    ] {
        app.database
            .collection::<Document>(name)
            .drop()
            .await
            .unwrap();
    }
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"open-access"}, doc! {"$set":{"version":1}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let preserved = app
        .database
        .collection::<Document>("menus")
        .find_one(doc! {"_id":menu})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        preserved.get_str("label").unwrap(),
        "Keep custom navigation"
    );
    assert!(!preserved.get_bool("enabled").unwrap());
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("api_tokens")
            .find_one(doc! {"_id":pat_id})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        app.database
            .collection::<Document>("_schema_migrations")
            .find_one(doc! {"_id":"open-access"})
            .await
            .unwrap()
            .unwrap()
            .get_i32("version")
            .unwrap(),
        3
    );
    let item = create(&app, token, "strict-game").await;
    let id = item["application"]["id"].as_str().unwrap();
    for body in [
        json!({"version":0,"name":"no"}),
        json!({"version":1,"code":"renamed"}),
        json!({"version":1,"allowedScopes":["users.read"]}),
        json!({"version":1,"allowedIps":["hostname"]}),
        json!({"version":1}),
    ] {
        assert_eq!(
            app.send(
                "PATCH",
                &format!("/api/v1/applications/{id}"),
                body,
                Some(token)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/applications/{id}"),
            json!({"version":2,"name":"stale"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for body in [
        json!({"username":"valid.user","displayName":"No","password":"short"}),
        json!({"username":"valid.user","displayName":"No","password":PASSWORD,"scopes":["open.users.read"]}),
        json!({"username":"valid.user","displayName":"No","password":PASSWORD,"roleIds":[]}),
    ] {
        assert_eq!(
            app.send(
                "POST",
                &format!("/api/v1/applications/{id}/users"),
                body,
                Some(token)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"open-access"}, doc! {"$set":{"version":999}})
        .await
        .unwrap();
    assert!(
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .is_err()
    );
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}

#[tokio::test]
async fn shared_applications_do_not_expose_another_actors_creation_or_rotation_receipts() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let created = create(&app, owner, "shared-game").await;
    let id = created["application"]["id"].as_str().unwrap();
    let original = created["application"]["requestId"].as_str().unwrap();
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"second-owner","displayName":"Another Owner","password":PASSWORD,"roleIds":auth["user"]["roleIds"]}),Some(owner)).await.0,StatusCode::CREATED);
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"second-owner","password":PASSWORD}),
            None,
        )
        .await;
    let other = login["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}"),
            Value::Null,
            Some(other)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/by-request/{original}"),
            Value::Null,
            Some(other)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let request = uuid::Uuid::new_v4().to_string();
    app.send(
        "POST",
        &format!("/api/v1/applications/{id}/secret"),
        json!({"version":1,"requestId":request}),
        Some(owner),
    )
    .await;
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/applications/{id}/actions/{request}"),
            Value::Null,
            Some(other)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.cleanup().await;
}

#[tokio::test]
async fn user_concurrency_paging_and_parent_fences_preserve_a_single_active_identity() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let created = create(&app, token, "paged-game").await;
    let id = created["application"]["id"].as_str().unwrap();
    let path = format!("/api/v1/applications/{id}/users");
    let body = json!({"username":"race.user","displayName":"Race","password":PASSWORD});
    let (one, two) = tokio::join!(
        app.send("POST", &path, body.clone(), Some(token)),
        app.send("POST", &path, body, Some(token))
    );
    assert!([one.0, two.0].contains(&StatusCode::CREATED));
    assert!([one.0, two.0].contains(&StatusCode::CONFLICT));
    for index in 0..21 {
        user(&app, token, id, &format!("paged.{index:02}")).await;
    }
    let (_, page) = app
        .send(
            "GET",
            &format!("{path}?page=2&pageSize=20&sort=username&search=paged."),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(page["meta"]["total"], 21);
    assert_eq!(page["data"].as_array().unwrap().len(), 1);
    assert_eq!(page["data"][0]["username"], "paged.20");
    let (_, meta) = app
        .send(
            "GET",
            &format!("/api/v1/applications/{id}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(meta["data"]["version"], 1);
    app.cleanup().await;
}
