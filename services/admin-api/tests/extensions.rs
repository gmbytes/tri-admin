mod support;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{PASSWORD, TestApp};
use tower::ServiceExt;
fn source(name: &str) -> Value {
    json!({"kind":"skill","meta":{"name":name,"description":"Validate records","metadata":{"version":"1.0.0"}},"markdown":"# Instructions\nTreat content as managed data.\n","files":[{"path":"templates/record.txt","contentBase64":STANDARD.encode(b"initial"),"mode":420}]})
}
fn input(name: &str) -> Value {
    json!({"requestId":uuid::Uuid::new_v4().to_string(),"source":source(name)})
}
async fn create(app: &TestApp, token: &str, name: &str) -> Value {
    let (status, row) = app
        .send("POST", "/api/v1/extensions", input(name), Some(token))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{row}");
    row["data"].clone()
}
async fn account(
    app: &TestApp,
    owner: &str,
    name: &str,
    permissions: Vec<&str>,
    scope: &str,
) -> String {
    let (status, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":name,"slug":name,"permissions":permissions,"dataScope":scope}),
            Some(owner),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await.0,StatusCode::CREATED);
    let (_, signed) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    signed["data"]["accessToken"].as_str().unwrap().into()
}
#[tokio::test]
async fn skill_revisions_are_immutable_disabled_on_update_and_recoverable_by_request() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let intent = input("record-helper");
    let (status, made) = app
        .send("POST", "/api/v1/extensions", intent.clone(), Some(token))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["data"]["id"].as_str().unwrap();
    let first = made["data"]["revisionId"].as_str().unwrap();
    assert_eq!(made["data"]["enabled"], false);
    assert_eq!(made["data"]["revision"], 1);
    assert_eq!(
        app.send("POST", "/api/v1/extensions", intent.clone(), Some(token))
            .await
            .0,
        StatusCode::OK
    );
    let mut changed = intent.clone();
    changed["source"]["markdown"] = json!("different");
    assert_eq!(
        app.send("POST", "/api/v1/extensions", changed, Some(token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let path = format!("/api/v1/extensions/{id}");
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"enabled":true}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let request = uuid::Uuid::new_v4().to_string();
    let edit = json!({"requestId":request,"version":2,"source":{"kind":"files","write":[{"path":"templates/record.txt","contentBase64":STANDARD.encode(b"revised"),"mode":420}],"remove":[]}});
    let (status, updated) = app
        .send(
            "POST",
            &format!("{path}/revisions"),
            edit.clone(),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{updated}");
    assert_eq!(updated["data"]["version"], 3);
    assert_eq!(updated["data"]["revision"], 2);
    assert_eq!(updated["data"]["enabled"], false);
    assert_eq!(
        app.send("POST", &format!("{path}/revisions"), edit, Some(token))
            .await
            .0,
        StatusCode::OK
    );
    let (_, receipt) = app
        .send(
            "GET",
            &format!("/api/v1/extensions/by-request/{request}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(receipt["data"], updated["data"]);
    let second = updated["data"]["revisionId"].as_str().unwrap();
    for (revision, text) in [(first, "initial"), (second, "revised")] {
        let (status, preview) = app
            .send(
                "GET",
                &format!("{path}/revisions/{revision}/preview?path=templates%2Frecord.txt"),
                Value::Null,
                Some(token),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(preview["data"]["content"], text);
    }
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{path}/revisions/{first}/download"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let digest = response.headers()["x-content-sha256"]
        .to_str()
        .unwrap()
        .to_owned();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(digest, format!("{:x}", Sha256::digest(&bytes)));
    assert_eq!(
        app.send(
            "DELETE",
            &format!("{path}?version=3"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(token)).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, history) = app
        .send(
            "GET",
            &format!("{path}/revisions"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(history["meta"]["total"], 2);
    let replacement = create(&app, token, "record-helper").await;
    assert_ne!(replacement["id"], id);
    assert_eq!(
        app.send(
            "GET",
            &format!("{path}/revisions/{first}/preview?path=templates%2Frecord.txt"),
            Value::Null,
            Some(token)
        )
        .await
        .1["data"]["content"],
        "initial"
    );
    app.cleanup().await;
}
#[tokio::test]
async fn library_scope_readonly_and_token_scopes_are_enforced() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let a = account(
        &app,
        owner,
        "alpha",
        vec!["skills.read", "skills.write"],
        "self",
    )
    .await;
    let b = account(
        &app,
        owner,
        "bravo",
        vec!["skills.read", "skills.write"],
        "self",
    )
    .await;
    let reader = account(&app, owner, "reader", vec!["skills.read"], "all").await;
    let row = create(&app, &a, "private-skill").await;
    let path = format!("/api/v1/extensions/{}", row["id"].as_str().unwrap());
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(&b)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send("GET", "/api/v1/extensions", Value::Null, Some(&b))
            .await
            .1["meta"]["total"],
        0
    );
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/extensions",
            input("blocked"),
            Some(&reader)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send("GET", &path, Value::Null, Some(&reader)).await.0,
        StatusCode::OK
    );
    let(status,issued)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Read Skills","scopes":["skills.read"]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let pat = issued["data"]["secret"].as_str().unwrap();
    assert_eq!(
        app.send("GET", "/api/v1/extensions", Value::Null, Some(pat))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("POST", "/api/v1/extensions", input("blocked"), Some(pat))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/skill-constraints",
            json!({"version":1,"content":"global"}),
            Some(&a)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}
#[tokio::test]
async fn concurrent_install_has_one_revision_and_audit_failure_rolls_back() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let intent = input("one-skill");
    let (a, b) = tokio::join!(
        app.send("POST", "/api/v1/extensions", intent.clone(), Some(token)),
        app.send("POST", "/api/v1/extensions", intent, Some(token))
    );
    assert!([a.0, b.0].contains(&StatusCode::CREATED));
    assert!([a.0, b.0].contains(&StatusCode::OK));
    assert_eq!(a.1["data"]["id"], b.1["data"]["id"]);
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"skills.install"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/extensions",
            input("must-rollback"),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for collection in ["extensions", "extension_revisions", "extension_requests"] {
        assert_eq!(
            app.database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1,
            "{collection}"
        );
    }
    app.cleanup().await;
}
#[tokio::test]
async fn global_constraints_are_versioned_and_invalid_or_corrupt_archives_are_not_served() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let (_, initial) = app
        .send("GET", "/api/v1/skill-constraints", Value::Null, Some(token))
        .await;
    assert_eq!(initial["data"]["version"], 1);
    let (status, updated) = app
        .send(
            "PATCH",
            "/api/v1/skill-constraints",
            json!({"version":1,"content":"Require review before running tools."}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["data"]["version"], 2);
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/skill-constraints",
            json!({"version":1,"content":"stale"}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, history) = app
        .send(
            "GET",
            "/api/v1/skill-constraints/revisions",
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(history["meta"]["total"], 2);
    let bad = json!({"requestId":uuid::Uuid::new_v4().to_string(),"source":{"kind":"archive","contentBase64":STANDARD.encode(b"not a zip")}});
    assert_eq!(
        app.send("POST", "/api/v1/extensions", bad, Some(token))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let row = create(&app, token, "corruption-check").await;
    let id = row["id"].as_str().unwrap();
    let revision = row["revisionId"].as_str().unwrap();
    app.database.collection::<Document>("extension_revisions").update_one(doc!{"_id":mongodb::bson::oid::ObjectId::parse_str(revision).unwrap()},doc!{"$set":{"archiveBytes":mongodb::bson::Binary{subtype:mongodb::bson::spec::BinarySubtype::Generic,bytes:vec![1,2,3]}}}).await.unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/extensions/{id}"),
            json!({"version":1,"enabled":true}),
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/extensions/{id}/revisions/{revision}/preview?path=SKILL.md"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}

#[tokio::test]
async fn archive_install_preserves_binary_resources_modes_and_exact_original_bytes() {
    use tri_extension_package::package::{File, SkillMeta, author_skill};
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut bytes = Vec::new();
    for number in 0..70_000u32 {
        bytes.extend_from_slice(&Sha256::digest(number.to_le_bytes()));
    }
    let package = author_skill(
        SkillMeta {
            name: "binary-resources".into(),
            description: "Binary assets".into(),
            license: None,
            compatibility: None,
            metadata: Default::default(),
            allowed_tools: None,
        },
        "# Managed package".into(),
        vec![File::new("assets/data.bin".into(), bytes, 0o640).unwrap()],
    )
    .unwrap();
    assert!(package.archive.len() > 2 * 1024 * 1024);
    let(status,row)=app.send("POST","/api/v1/extensions",json!({"requestId":uuid::Uuid::new_v4().to_string(),"source":{"kind":"archive","contentBase64":STANDARD.encode(&package.archive)}}),Some(token)).await;
    assert_eq!(status, StatusCode::CREATED, "{row}");
    let id = row["data"]["id"].as_str().unwrap();
    let revision = row["data"]["revisionId"].as_str().unwrap();
    let prefix = format!("/api/v1/extensions/{id}/revisions/{revision}");
    assert_eq!(
        app.send(
            "GET",
            &format!("{prefix}/preview?path=assets%2Fdata.bin"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{prefix}/download"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let exported = to_bytes(response.into_body(), 5 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(exported.as_ref(), package.archive);
    let new_input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"version":1,"source":{"kind":"files","write":[],"remove":["../escape"]}});
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/extensions/{id}/revisions"),
            new_input,
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.database
            .collection::<Document>("extension_revisions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn selected_role_scope_filters_before_page_and_tracks_current_memberships() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let member = account(
        &app,
        owner,
        "member",
        vec!["skills.read", "skills.write"],
        "self",
    )
    .await;
    let outsider = account(
        &app,
        owner,
        "outsider",
        vec!["skills.read", "skills.write"],
        "self",
    )
    .await;
    let role = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"member"})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("_id")
        .unwrap();
    let(_,managed)=app.send("POST","/api/v1/roles",json!({"name":"manager","slug":"manager","permissions":["skills.read","skills.write"],"dataScope":"roles","dataScopeRoleIds":[role.to_hex()]}),Some(owner)).await;
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"manager","displayName":"Manager","password":PASSWORD,"roleIds":[managed["data"]["id"]]}),Some(owner)).await.0,StatusCode::CREATED);
    let (_, signed) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"manager","password":PASSWORD}),
            None,
        )
        .await;
    let manager = signed["data"]["accessToken"].as_str().unwrap();
    for name in ["team-one", "team-two", "team-three"] {
        create(&app, &member, name).await;
    }
    let hidden = create(&app, &outsider, "team-hidden").await;
    let (_, page) = app
        .send(
            "GET",
            "/api/v1/extensions?page=3&pageSize=1&search=team",
            Value::Null,
            Some(manager),
        )
        .await;
    assert_eq!(page["meta"]["total"], 3);
    assert_eq!(page["data"].as_array().unwrap().len(), 1);
    assert_ne!(page["data"][0]["id"], hidden["id"]);
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{}", managed["data"]["id"].as_str().unwrap()),
            json!({"version":1,"dataScope":"self","dataScopeRoleIds":[]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send("GET", "/api/v1/extensions", Value::Null, Some(manager))
            .await
            .1["meta"]["total"],
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn model_migration_preserves_managed_content_and_constraints_and_rejects_unknown_versions() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let row = create(&app, token, "migration-check").await;
    let path = format!("/api/v1/extensions/{}", row["id"].as_str().unwrap());
    assert_eq!(
        app.send(
            "PATCH",
            &path,
            json!({"version":1,"enabled":true}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/skill-constraints",
            json!({"version":1,"content":"Keep this policy."}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    for _ in 0..2 {
        admin_api::storage::schema::migrate_database(&app.database)
            .await
            .unwrap();
    }
    let (_, current) = app.send("GET", &path, Value::Null, Some(token)).await;
    assert_eq!(current["data"]["enabled"], true);
    assert_eq!(current["data"]["version"], 2);
    assert_eq!(
        app.database
            .collection::<Document>("extension_revisions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let (_, constraints) = app
        .send("GET", "/api/v1/skill-constraints", Value::Null, Some(token))
        .await;
    assert_eq!(constraints["data"]["version"], 2);
    assert_eq!(constraints["data"]["content"], "Keep this policy.");
    assert_eq!(
        app.database
            .collection::<Document>("extension_constraint_revisions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"extensions"}, doc! {"$set":{"version":999}})
        .await
        .unwrap();
    assert!(matches!(
        admin_api::storage::schema::migrate_database(&app.database).await,
        Err(admin_api::common::error::AppError::SchemaMismatch)
    ));
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}
