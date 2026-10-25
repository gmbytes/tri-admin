mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::{PASSWORD, TestApp};

#[tokio::test]
async fn settings_menu_requires_v3_and_preserves_customization_and_deletion() {
    let app = TestApp::new().await;
    let menus = app.database.collection::<Document>("menus");
    menus.delete_many(doc! {"path":"/settings"}).await.unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"settings"}, doc! {"$set":{"version":2}})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let row = menus
        .find_one(doc! {"path":"/settings","deletedAt":null})
        .await
        .unwrap()
        .unwrap();
    let id = row.get_object_id("_id").unwrap();
    assert_eq!(row.get_str("viewKey").unwrap(), "settings");
    assert_eq!(row.get_str("permission").unwrap(), "settings.read");
    menus
        .update_one(doc! {"_id":id}, doc! {"$set":{"label":"自定义配置中心"}})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        menus
            .find_one(doc! {"_id":id})
            .await
            .unwrap()
            .unwrap()
            .get_str("label")
            .unwrap(),
        "自定义配置中心"
    );
    menus
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"deletedAt":mongodb::bson::DateTime::now()}},
        )
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        menus
            .count_documents(doc! {"path":"/settings","deletedAt":null})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::OK
    );
    app.cleanup().await;
}

fn http(code: &str) -> Value {
    json!({"name":"External","code":code,"enabled":true,"configuration":{"kind":"http","baseUrl":"https://api.example.test/v1"},"secret":{"kind":"http","headerName":"Authorization","value":"Bearer PRIVATE-CONFIG-TOKEN"}})
}
async fn post(app: &TestApp, token: &str, input: Value) -> Value {
    let (status, result) = app
        .send("POST", "/api/v1/settings/profiles", input, Some(token))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    result["data"].clone()
}
async fn identity(app: &TestApp, owner: &str) -> String {
    let(_,role)=app.send("POST","/api/v1/roles",json!({"name":"settings-viewer","slug":"settings-viewer","permissions":["settings.read"],"dataScope":"self"}),Some(owner)).await;
    let(status,result)=app.send("POST","/api/v1/users",json!({"username":"settings-viewer","displayName":"Settings viewer","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    let (_, login) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"settings-viewer","password":PASSWORD}),
            None,
        )
        .await;
    login["data"]["accessToken"].as_str().unwrap().into()
}

#[tokio::test]
async fn runtime_and_service_settings_are_safe_gated_and_live_login_policy_can_be_reset() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (status, runtime) = app
        .send("GET", "/api/v1/settings/runtime", Value::Null, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!runtime.to_string().contains(support::KEY));
    assert!(!runtime.to_string().contains(&"11".repeat(32)));
    assert!(runtime["data"].get("mongodbUri").is_none());
    let (status, current) = app
        .send("GET", "/api/v1/settings/service", Value::Null, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    let version = current["data"]["version"].as_i64().unwrap();
    let (status, changed) = app
        .send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":version,"displayName":"Game Operations","loginLimitPerIpOverride":2}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["data"]["effectiveLoginLimitPerIp"], 2);
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"absent-one","password":"incorrect"}),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"absent-two","password":"incorrect"}),
            None
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let (status, _) = app
        .send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":version+1,"loginLimitPerIpOverride":null}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, info) = app
        .send("GET", "/api/v1/application", Value::Null, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["data"]["displayName"], "Game Operations");
    assert_eq!(app.login().await["user"]["username"], "admin");
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":version,"displayName":"stale"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send("GET", "/api/v1/settings/runtime", Value::Null, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let viewer = identity(&app, token).await;
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/settings/service",
            Value::Null,
            Some(&viewer)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/settings/profiles",
            http("denied"),
            Some(&viewer)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}

#[tokio::test]
async fn profile_credentials_are_encrypted_never_returned_and_omission_null_and_replacement_are_distinct()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let profile = post(&app, token, http("external")).await;
    let id = profile["id"].as_str().unwrap();
    assert_eq!(profile["credentials"]["configured"], true);
    assert_eq!(profile["credentials"]["keyId"], "fixture");
    assert!(!profile.to_string().contains("PRIVATE-CONFIG-TOKEN"));
    assert!(profile.get("secret").is_none());
    let stored = app
        .database
        .collection::<Document>("integration_profiles")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{stored:?}").contains("PRIVATE-CONFIG-TOKEN"));
    let first = stored.get_document("secretBox").unwrap().clone();
    let (status, _) = app
        .send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{id}"),
            json!({"version":1,"name":"Renamed"}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let kept = app
        .database
        .collection::<Document>("integration_profiles")
        .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.get_document("secretBox").unwrap(), &first);
    assert_eq!(app.send("PATCH",&format!("/api/v1/settings/profiles/{id}"),json!({"version":2,"configuration":{"kind":"http","baseUrl":"https://different.example.test"}}),Some(token)).await.0,StatusCode::CONFLICT);
    let(status,_)=app.send("PATCH",&format!("/api/v1/settings/profiles/{id}"),json!({"version":2,"secret":{"kind":"http","headerName":"X-API-Key","value":"ANOTHER-PRIVATE-TOKEN"}}),Some(token)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, cleared) = app
        .send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{id}"),
            json!({"version":3,"secret":null}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["data"]["credentials"]["configured"], false);
    let (_, logs) = app
        .send(
            "GET",
            "/api/v1/logs?kind=operation&action=settings.profiles.update",
            Value::Null,
            Some(token),
        )
        .await;
    assert!(!logs.to_string().contains("PRIVATE-TOKEN"));
    app.cleanup().await;
}

#[tokio::test]
async fn typed_integrations_reject_credential_urls_traversal_unknown_fields_and_unsafe_secret_types()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    for (configuration, secret) in [
        (
            json!({"kind":"smtp","host":"127.0.0.1","port":2525,"security":"plain","fromAddress":"ops@::1"}),
            Value::Null,
        ),
        (
            json!({"kind":"http","baseUrl":"https://user:password@example.test"}),
            Value::Null,
        ),
        (
            json!({"kind":"http","baseUrl":"http://example.test"}),
            Value::Null,
        ),
        (
            json!({"kind":"http","baseUrl":"https://example.test?token=private"}),
            Value::Null,
        ),
        (
            json!({"kind":"localStorage","subdirectory":"../outside"}),
            Value::Null,
        ),
        (
            json!({"kind":"localStorage","subdirectory":"C:\\outside"}),
            Value::Null,
        ),
        (
            json!({"kind":"localStorage","subdirectory":"CON.txt"}),
            Value::Null,
        ),
        (
            json!({"kind":"http","baseUrl":"https://example.test"}),
            json!({"kind":"smtp","username":"u","password":"private"}),
        ),
        (
            json!({"kind":"http","baseUrl":"https://example.test"}),
            json!({"kind":"http","headerName":"Host","value":"private"}),
        ),
        (
            json!({"kind":"http","baseUrl":"https://example.test"}),
            json!({"kind":"http","headerName":"Authorization","value":"bad\r\nheader"}),
        ),
        (
            json!({"kind":"mongodb","hosts":[{"host":"db.example.test","port":27017}],"database":"game","authDatabase":"admin","tls":false,"directConnection":true}),
            json!({"kind":"mongodb","username":"u","password":"p"}),
        ),
    ] {
        let(status,body)=app.send("POST","/api/v1/settings/profiles",json!({"name":"Invalid","code":"invalid","enabled":false,"configuration":configuration,"secret":secret}),Some(token)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    let mut value = http("unknown");
    value["password"] = json!("hidden");
    assert_eq!(
        app.send("POST", "/api/v1/settings/profiles", value, Some(token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.cleanup().await;
}

#[tokio::test]
async fn bindings_types_versions_and_storage_location_are_protected_and_active_codes_are_unique() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (_, service) = app
        .send("GET", "/api/v1/settings/service", Value::Null, Some(token))
        .await;
    let old = service["data"]["storageProfileId"].as_str().unwrap();
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/settings/profiles/{old}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let local=post(&app,token,json!({"name":"Storage","code":"second-storage","enabled":true,"configuration":{"kind":"localStorage","subdirectory":"second"}})).await;
    let id = local["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{id}"),
            json!({"version":1,"configuration":{"kind":"localStorage","subdirectory":"moved"}}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let smtp=post(&app,token,json!({"name":"SMTP","code":"mail","enabled":true,"configuration":{"kind":"smtp","host":"127.0.0.1","port":2525,"security":"plain","fromAddress":"ops@example.test"}})).await;
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":1,"storageProfileId":smtp["id"]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":1,"storageProfileId":id,"mailProfileId":smtp["id"]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/settings/profiles/{id}"),
            json!({"version":1,"enabled":false}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/settings/profiles/{old}?version=1"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let input = http("concurrent");
    let (a, b) = tokio::join!(
        app.send(
            "POST",
            "/api/v1/settings/profiles",
            input.clone(),
            Some(token)
        ),
        app.send("POST", "/api/v1/settings/profiles", input, Some(token))
    );
    let statuses = [a.0, b.0];
    assert!(statuses.contains(&StatusCode::CREATED) && statuses.contains(&StatusCode::CONFLICT));
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/settings/profiles?pageSize=101",
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.cleanup().await;
}

#[tokio::test]
async fn tampered_or_swapped_ciphertext_cannot_be_rewrapped_and_result_failure_rolls_back() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let first = post(&app, token, http("first")).await;
    let second = post(&app, token, http("second")).await;
    let a = ObjectId::parse_str(first["id"].as_str().unwrap()).unwrap();
    let b = ObjectId::parse_str(second["id"].as_str().unwrap()).unwrap();
    let source = app
        .database
        .collection::<Document>("integration_profiles")
        .find_one(doc! {"_id":a})
        .await
        .unwrap()
        .unwrap();
    app.database
        .collection::<Document>("integration_profiles")
        .update_one(
            doc! {"_id":b},
            doc! {"$set":{"secretBox":source.get_document("secretBox").unwrap().clone()}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/settings/profiles/{}/rewrap", b.to_hex()),
            json!({"version":1}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (status, _) = app
        .send(
            "POST",
            &format!("/api/v1/settings/profiles/{}/rewrap", a.to_hex()),
            json!({"version":1}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let changed = app
        .database
        .collection::<Document>("integration_profiles")
        .find_one(doc! {"_id":a})
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        changed.get_document("secretBox").unwrap(),
        source.get_document("secretBox").unwrap()
    );
    let config_json = admin_api::common::canonical::json(
        &json!({"kind":"http","baseUrl":"https://changed.example.test"}),
    )
    .unwrap();
    app.database
        .collection::<Document>("integration_profiles")
        .update_one(
            doc! {"_id":a},
            doc! {"$set":{"configurationJson":config_json}},
        )
        .await
        .unwrap();
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/settings/profiles/{}/rewrap", a.to_hex()),
            json!({"version":2}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"$jsonSchema":{"required":["rejectFixture"]}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/settings/profiles",
            http("rollback"),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.database
            .collection::<Document>("integration_profiles")
            .count_documents(doc! {"code":"rollback"})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

fn configured_router(app: &TestApp, keys: Option<String>, active: Option<&str>) -> TestApp {
    let config = admin_api::config::Config::from_lookup(|name| match name {
        "TRI_JWT_SIGNING_KEY" => Some(support::KEY.into()),
        "TRI_SETTINGS_KEYS" => keys.clone(),
        "TRI_SETTINGS_ACTIVE_KEY" => active.map(str::to_owned),
        _ => None,
    })
    .unwrap();
    TestApp {
        router: admin_api::build_router(admin_api::state::AppState::new(
            config,
            Some(app.database.clone()),
        )),
        database: app.database.clone(),
    }
}
#[tokio::test]
async fn key_rotation_retains_old_decryption_keys_and_missing_keys_never_fall_back_to_plaintext() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = post(&app, token, http("rotation")).await;
    let id = row["id"].as_str().unwrap();
    let rotated = configured_router(
        &app,
        Some(format!(
            "fixture:{},rotated:{}",
            "11".repeat(32),
            "22".repeat(32)
        )),
        Some("rotated"),
    );
    let (status, changed) = rotated
        .send(
            "POST",
            &format!("/api/v1/settings/profiles/{id}/rewrap"),
            json!({"version":1}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["data"]["credentials"]["keyId"], "rotated");
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/settings/profiles/{id}/rewrap"),
            json!({"version":2}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let only_new = configured_router(
        &app,
        Some(format!("rotated:{}", "22".repeat(32))),
        Some("rotated"),
    );
    assert_eq!(
        only_new
            .send(
                "POST",
                &format!("/api/v1/settings/profiles/{id}/rewrap"),
                json!({"version":2}),
                Some(token)
            )
            .await
            .0,
        StatusCode::OK
    );
    let disabled = configured_router(&app, None, None);
    let (status, body) = disabled
        .send(
            "POST",
            "/api/v1/settings/profiles",
            http("no-key"),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "SECRET_STORE_UNAVAILABLE");
    assert_eq!(
        app.database
            .collection::<Document>("integration_profiles")
            .count_documents(doc! {"code":"no-key"})
            .await
            .unwrap(),
        0
    );
    let mut anonymous = http("anonymous");
    anonymous.as_object_mut().unwrap().remove("secret");
    assert_eq!(
        disabled
            .send("POST", "/api/v1/settings/profiles", anonymous, Some(token))
            .await
            .0,
        StatusCode::CREATED
    );
    let mail=post(&app,token,json!({"name":"Mail","code":"bound-mail","enabled":true,"configuration":{"kind":"smtp","host":"127.0.0.1","port":2525,"security":"plain","fromAddress":"ops@example.test"},"secret":{"kind":"smtp","username":"fixture","password":"fixture-password"}})).await;
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":1,"mailProfileId":mail["id"]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        disabled
            .send(
                "PATCH",
                "/api/v1/settings/service",
                json!({"version":2,"displayName":"Core settings remain editable"}),
                Some(token)
            )
            .await
            .0,
        StatusCode::OK
    );
    app.cleanup().await;
}

#[tokio::test]
async fn every_integration_kind_has_a_valid_typed_configuration_and_bounded_queries() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    for (code, configuration, secret) in [
        (
            "mongo",
            json!({"kind":"mongodb","hosts":[{"host":"127.0.0.1","port":13317}],"database":"game","authDatabase":"admin","replicaSet":"tri-admin-rs","tls":false,"directConnection":true}),
            json!({"kind":"mongodb","username":"fixture-user","password":"fixture-password"}),
        ),
        (
            "objects",
            json!({"kind":"s3","endpoint":"http://127.0.0.1:9900","bucket":"game-files","region":"us-east-1","prefix":"assets/v1","pathStyle":true}),
            json!({"kind":"s3","accessKeyId":"fixture-key","secretAccessKey":"fixture-password","sessionToken":"fixture-session-token"}),
        ),
        (
            "mail-auth",
            json!({"kind":"smtp","host":"127.0.0.1","port":2525,"security":"plain","fromAddress":"ops@example.test"}),
            json!({"kind":"smtp","username":"fixture-user","password":"fixture-password"}),
        ),
    ] {
        let row=post(&app,token,json!({"name":code,"code":code,"enabled":true,"configuration":configuration,"secret":secret})).await;
        assert_eq!(row["credentials"]["configured"], true);
        assert!(!row.to_string().contains("fixture-password"));
    }
    let (status, result) = app
        .send(
            "GET",
            "/api/v1/settings/profiles?kind=s3&pageSize=1&sort=code",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["meta"]["total"], 1);
    assert_eq!(result["data"][0]["code"], "objects");
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/settings/profiles?kind=mysql",
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.database
        .collection::<Document>("_schema_migrations")
        .delete_one(doc! {"_id":"settings"})
        .await
        .unwrap();
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}
