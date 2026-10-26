mod support;
use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{PASSWORD, TestApp};
use tower::ServiceExt;
fn descriptor(label: &str) -> Value {
    json!({"resource":"inventory_items","label":label,"fields":[{"name":"sku","label":"编号","kind":"text","required":true,"searchable":true,"unique":true},{"name":"count","label":"数量","kind":"integer","required":true}]})
}
async fn definition(app: &TestApp, token: &str, label: &str) -> Value {
    let (status, value) = app
        .send(
            "POST",
            "/api/v1/codegen/definitions",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"schema":descriptor(label)}),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["data"].clone()
}
async fn raw(app: &TestApp, path: &str, token: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}
#[tokio::test]
async fn definitions_and_immutable_generation_receipts_survive_edits_and_parent_deletion() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let input = json!({"requestId":request,"schema":descriptor("库存初版")});
    let (status, created) = app
        .send(
            "POST",
            "/api/v1/codegen/definitions",
            input.clone(),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send("POST", "/api/v1/codegen/definitions", input, Some(token))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/codegen/definitions/{id}"),
            json!({"version":1,"schema":descriptor("库存二版")}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/codegen/definitions/{id}"),
            json!({"version":1,"schema":descriptor("过期")}),
            Some(token)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let generation_request = uuid::Uuid::new_v4().to_string();
    let intent = json!({"requestId":generation_request,"definitionId":id,"definitionVersion":2});
    let (status, generated) = app
        .send(
            "POST",
            "/api/v1/codegen/generations",
            intent.clone(),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{generated}");
    let generation_id = generated["data"]["id"].as_str().unwrap();
    assert_eq!(generated["data"]["schema"]["label"], "库存二版");
    assert_eq!(generated["data"]["templateVersion"], 2);
    assert!(generated["data"]["files"].as_array().unwrap().len() > 25);
    assert!(!generated.to_string().contains("archiveBytes"));
    assert!(!generated.to_string().contains("fingerprint"));
    let (status, headers, bytes) = raw(
        &app,
        &format!("/api/v1/codegen/generations/{generation_id}/download"),
        token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/zip");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        generated["data"]["sha256"].as_str().unwrap()
    );
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let mut schema = String::new();
    use std::io::Read;
    archive
        .by_name("schema.json")
        .unwrap()
        .read_to_string(&mut schema)
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&schema).unwrap()["label"],
        "库存二版"
    );
    let (_, preview) = app
        .send(
            "GET",
            &format!(
                "/api/v1/codegen/generations/{generation_id}/preview?path=web%2Fsrc%2Fschema.json"
            ),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(preview["data"]["content"].as_str().unwrap()).unwrap()["label"],
        "库存二版"
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/codegen/generations/{generation_id}/preview?path=..%2F.env"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/codegen/definitions/{id}"),
            json!({"version":2,"schema":descriptor("库存三版")}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/codegen/definitions/{id}?version=3"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (status, replay) = app
        .send(
            "POST",
            "/api/v1/codegen/generations",
            intent.clone(),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["id"], generation_id);
    assert_eq!(replay["data"]["schema"]["label"], "库存二版");
    let mut mismatch = intent;
    mismatch["definitionVersion"] = json!(3);
    assert_eq!(
        app.send("POST", "/api/v1/codegen/generations", mismatch, Some(token))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (_, recovered) = app
        .send(
            "GET",
            &format!("/api/v1/codegen/generations/by-request/{generation_request}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(recovered["data"]["id"], generation_id);
    assert_eq!(
        raw(
            &app,
            &format!("/api/v1/codegen/generations/{generation_id}/download"),
            token
        )
        .await
        .2,
        bytes
    );
    let (_, receipt) = app
        .send(
            "GET",
            &format!("/api/v1/codegen/definitions/by-request/{request}"),
            Value::Null,
            Some(token),
        )
        .await;
    assert!(!receipt["data"]["deletedAt"].is_null());
    assert_eq!(
        app.database
            .collection::<Document>("code_generations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"codegen.generate"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
async fn account(
    app: &TestApp,
    owner: &str,
    name: &str,
    permissions: Vec<&str>,
    scope: &str,
) -> String {
    let (_, role) = app
        .send(
            "POST",
            "/api/v1/roles",
            json!({"name":name,"slug":name,"permissions":permissions,"dataScope":scope}),
            Some(owner),
        )
        .await;
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await.0,StatusCode::CREATED);
    let (status, auth) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    auth["data"]["accessToken"].as_str().unwrap().into()
}
#[tokio::test]
async fn codegen_scope_readonly_and_api_tokens_use_the_current_authority() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let a = account(
        &app,
        owner,
        "writer-a",
        vec!["codegen.read", "codegen.write"],
        "self",
    )
    .await;
    let b = account(
        &app,
        owner,
        "writer-b",
        vec!["codegen.read", "codegen.write"],
        "self",
    )
    .await;
    let reader = account(&app, owner, "reader", vec!["codegen.read"], "all").await;
    let first = definition(&app, &a, "自己的定义").await;
    let id = first["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/codegen/definitions/{id}"),
            Value::Null,
            Some(&b)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.send("POST","/api/v1/codegen/generations",json!({"requestId":uuid::Uuid::new_v4().to_string(),"definitionId":id,"definitionVersion":1}),Some(&b)).await.0,StatusCode::NOT_FOUND);
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/codegen/definitions",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"schema":descriptor("只读禁止")}),
            Some(&reader)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/codegen/definitions/{id}"),
            Value::Null,
            Some(&reader)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, list) = app
        .send("GET", "/api/v1/codegen/definitions", Value::Null, Some(&b))
        .await;
    assert_eq!(list["meta"]["total"], 0);
    let(status,issued)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Code automation","scopes":["codegen.read","codegen.write"]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED);
    let pat = issued["data"]["secret"].as_str().unwrap();
    let created = definition(&app, pat, "自动化定义").await;
    let(status,_)=app.send("POST","/api/v1/codegen/generations",json!({"requestId":uuid::Uuid::new_v4().to_string(),"definitionId":created["id"],"definitionVersion":1}),Some(pat)).await;
    assert_eq!(status, StatusCode::CREATED);
    let(_,limited)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Read only","scopes":["codegen.read"]}),Some(owner)).await;
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/codegen/definitions",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"schema":descriptor("越权")}),
            limited["data"]["secret"].as_str()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}
#[tokio::test]
async fn concurrent_generation_is_one_package_and_audit_failure_is_atomic() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let def = definition(&app, token, "并发生成").await;
    let input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"definitionId":def["id"],"definitionVersion":1});
    let (left, right) = tokio::join!(
        app.send(
            "POST",
            "/api/v1/codegen/generations",
            input.clone(),
            Some(token)
        ),
        app.send("POST", "/api/v1/codegen/generations", input, Some(token))
    );
    assert_eq!(
        [left.0, right.0]
            .iter()
            .filter(|status| **status == StatusCode::CREATED)
            .count(),
        1
    );
    assert_eq!(left.1["data"]["id"], right.1["data"]["id"]);
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"codegen.generate"}},"validationLevel":"strict"}).await.unwrap();
    assert_eq!(app.send("POST","/api/v1/codegen/generations",json!({"requestId":uuid::Uuid::new_v4().to_string(),"definitionId":def["id"],"definitionVersion":1}),Some(token)).await.0,StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        app.database
            .collection::<Document>("code_generations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("codegen_packages")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}

#[tokio::test]
async fn role_scope_is_applied_before_pagination_and_changes_without_relogin() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let member = account(
        &app,
        owner,
        "team-member",
        vec!["codegen.read", "codegen.write"],
        "self",
    )
    .await;
    let outsider = account(
        &app,
        owner,
        "outsider",
        vec!["codegen.read", "codegen.write"],
        "self",
    )
    .await;
    let team = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"team-member"})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("_id")
        .unwrap();
    let(_,role)=app.send("POST","/api/v1/roles",json!({"name":"manager","slug":"manager","permissions":["codegen.read","codegen.write"],"dataScope":"roles","dataScopeRoleIds":[team.to_hex()]}),Some(owner)).await;
    assert_eq!(app.send("POST","/api/v1/users",json!({"username":"manager","displayName":"Manager","password":PASSWORD,"roleIds":[role["data"]["id"]]}),Some(owner)).await.0,StatusCode::CREATED);
    let (_, signed) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"manager","password":PASSWORD}),
            None,
        )
        .await;
    let manager = signed["data"]["accessToken"].as_str().unwrap();
    for index in 0..21 {
        let mut schema = descriptor("Team");
        schema["resource"] = json!(format!("team_{index}"));
        assert_eq!(
            app.send(
                "POST",
                "/api/v1/codegen/definitions",
                json!({"requestId":uuid::Uuid::new_v4().to_string(),"schema":schema}),
                Some(&member)
            )
            .await
            .0,
            StatusCode::CREATED
        );
    }
    let hidden = definition(&app, &outsider, "Other").await;
    let (_, page) = app
        .send(
            "GET",
            "/api/v1/codegen/definitions?page=2&pageSize=20",
            Value::Null,
            Some(manager),
        )
        .await;
    assert_eq!(page["meta"]["total"], 21);
    assert_eq!(page["data"].as_array().unwrap().len(), 1);
    assert_eq!(
        app.send(
            "GET",
            &format!(
                "/api/v1/codegen/definitions/{}",
                hidden["id"].as_str().unwrap()
            ),
            Value::Null,
            Some(manager)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{}", role["data"]["id"].as_str().unwrap()),
            json!({"version":1,"dataScope":"self","dataScopeRoleIds":[]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, changed) = app
        .send(
            "GET",
            "/api/v1/codegen/definitions",
            Value::Null,
            Some(manager),
        )
        .await;
    assert_eq!(changed["meta"]["total"], 0);
    app.cleanup().await;
}
#[tokio::test]
async fn cached_authority_never_bypasses_live_role_token_or_model_checks() {
    use admin_api::{
        common::{error::AppError, pagination::PageInput},
        config::Config,
        modules::{
            auth::{self, AdminCredential, AuthContext},
            codegen::{self, dto::CreateDefinition},
        },
        state::AppState,
    };
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let writer = account(
        &app,
        owner,
        "cached-writer",
        vec!["codegen.read", "codegen.write"],
        "self",
    )
    .await;
    let state = AppState::new(
        Config::from_lookup(|key| {
            if key == "TRI_JWT_SIGNING_KEY" {
                Some(support::KEY.into())
            } else {
                None
            }
        })
        .unwrap(),
        Some(app.database.clone()),
    );
    let cached = auth::service::authenticate(&state, &writer).await.unwrap();
    let role = app
        .database
        .collection::<Document>("roles")
        .find_one(doc! {"slug":"cached-writer"})
        .await
        .unwrap()
        .unwrap()
        .get_object_id("_id")
        .unwrap();
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{}", role.to_hex()),
            json!({"version":1,"permissions":["codegen.read"]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    let input = || {
        serde_json::from_value::<CreateDefinition>(
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"schema":descriptor("Cached")}),
        )
        .unwrap()
    };
    assert!(matches!(
        codegen::service::create_definition(&app.database, &cached, input()).await,
        Err(AppError::Forbidden)
    ));
    let(_,issued)=app.send("POST","/api/v1/api-tokens",json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Read metadata","scopes":["codegen.read"]}),Some(owner)).await;
    let context = AuthContext {
        user: auth::service::authenticate(&state, owner)
            .await
            .unwrap()
            .user,
        credential: AdminCredential::ApiToken {
            token_id: mongodb::bson::oid::ObjectId::parse_str(
                issued["data"]["token"]["id"].as_str().unwrap(),
            )
            .unwrap(),
        },
    };
    assert!(matches!(
        codegen::service::create_definition(&app.database, &context, input()).await,
        Err(AppError::Forbidden)
    ));
    app.database
        .collection::<Document>("_schema_migrations")
        .delete_one(doc! {"_id":"codegen"})
        .await
        .unwrap();
    assert!(matches!(
        codegen::service::definitions(
            &app.database,
            &cached,
            PageInput {
                page: 1,
                page_size: 20,
                search: None,
                sort: None
            }
        )
        .await,
        Err(AppError::NotReady)
    ));
    assert_eq!(
        app.send("GET", "/health/ready", Value::Null, None).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    app.cleanup().await;
}
#[tokio::test]
async fn invalid_definitions_paths_and_corrupted_packages_are_rejected_without_private_data() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let mut bad = descriptor("Bad");
    bad["sql"] = json!("must never be accepted");
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/codegen/definitions",
            json!({"requestId":uuid::Uuid::new_v4().to_string(),"schema":bad}),
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let def = definition(&app, token, "Integrity").await;
    let(_,made)=app.send("POST","/api/v1/codegen/generations",json!({"requestId":uuid::Uuid::new_v4().to_string(),"definitionId":def["id"],"definitionVersion":1}),Some(token)).await;
    let id = made["data"]["id"].as_str().unwrap();
    assert_eq!(
        app.send(
            "GET",
            &format!("/api/v1/codegen/generations/{id}/preview?path=private.txt"),
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/codegen/definitions?ownerId=000000000000000000000000",
            Value::Null,
            Some(token)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    app.database.collection::<Document>("codegen_packages").update_one(doc!{"_id":mongodb::bson::oid::ObjectId::parse_str(id).unwrap()},doc!{"$set":{"archiveBytes":mongodb::bson::Binary{subtype:mongodb::bson::spec::BinarySubtype::Generic,bytes:vec![1,2,3]}}}).await.unwrap();
    let (status, _, bytes) = raw(
        &app,
        &format!("/api/v1/codegen/generations/{id}/download"),
        token,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "SERVICE_NOT_READY");
    app.database
        .collection::<Document>("_schema_migrations")
        .update_one(doc! {"_id":"codegen"}, doc! {"$set":{"version":999}})
        .await
        .unwrap();
    assert!(matches!(
        admin_api::storage::schema::migrate_database(&app.database).await,
        Err(admin_api::common::error::AppError::SchemaMismatch)
    ));
    app.cleanup().await;
}
#[tokio::test]
async fn older_template_history_downloads_original_bytes_after_a_new_template_is_current() {
    use std::io::Write;
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let def = definition(&app, token, "Historical").await;
    let intent = json!({"requestId":uuid::Uuid::new_v4().to_string(),"definitionId":def["id"],"definitionVersion":1});
    let (_, made) = app
        .send(
            "POST",
            "/api/v1/codegen/generations",
            intent.clone(),
            Some(token),
        )
        .await;
    let id = made["data"]["id"].as_str().unwrap();
    let oid = mongodb::bson::oid::ObjectId::parse_str(id).unwrap();
    let old_content = "Original older template source";
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("README.md", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(old_content.as_bytes()).unwrap();
    let old_zip = zip.finish().unwrap().into_inner();
    let sha = format!("{:x}", Sha256::digest(&old_zip));
    let file_sha = format!("{:x}", Sha256::digest(old_content.as_bytes()));
    app.database.collection::<Document>("code_generations").update_one(doc!{"_id":oid},doc!{"$set":{"templateVersion":1,"sha256":&sha,"size":old_zip.len() as i64,"files":[{"path":"README.md","size":old_content.len() as i64,"sha256":file_sha}]}}).await.unwrap();
    app.database.collection::<Document>("codegen_packages").update_one(doc!{"_id":oid},doc!{"$set":{"archiveBytes":mongodb::bson::Binary{subtype:mongodb::bson::spec::BinarySubtype::Generic,bytes:old_zip.clone()}}}).await.unwrap();
    let (status, replay) = app
        .send("POST", "/api/v1/codegen/generations", intent, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["templateVersion"], 1);
    assert_eq!(replay["data"]["sha256"], sha);
    assert_eq!(
        raw(
            &app,
            &format!("/api/v1/codegen/generations/{id}/download"),
            token
        )
        .await
        .2,
        old_zip
    );
    let (_, preview) = app
        .send(
            "GET",
            &format!("/api/v1/codegen/generations/{id}/preview?path=README.md"),
            Value::Null,
            Some(token),
        )
        .await;
    assert_eq!(preview["data"]["content"], old_content);
    app.cleanup().await;
}
#[tokio::test]
async fn codegen_navigation_migration_preserves_custom_and_deleted_menu_entries() {
    let app = TestApp::new().await;
    let menus = app.database.collection::<Document>("menus");
    let menu = menus
        .find_one(doc! {"path":"/codegen"})
        .await
        .unwrap()
        .unwrap();
    let id = menu.get_object_id("_id").unwrap();
    let now = mongodb::bson::DateTime::now();
    menus
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"label":"Custom generator","enabled":false,"deletedAt":now}},
        )
        .await
        .unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .delete_one(doc! {"_id":"codegen"})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    let retained = menus.find_one(doc! {"_id":id}).await.unwrap().unwrap();
    assert_eq!(retained.get_str("label").unwrap(), "Custom generator");
    assert_eq!(retained.get_datetime("deletedAt").unwrap(), &now);
    assert_eq!(
        menus
            .count_documents(doc! {"path":"/codegen"})
            .await
            .unwrap(),
        1
    );
    menus.delete_one(doc! {"_id":id}).await.unwrap();
    menus.insert_one(doc!{"path":"/codegen","label":"Occupied","viewKey":"dashboard","parentId":null,"permission":null,"sort":140,"enabled":true,"deletedAt":null,"version":7i64,"createdAt":now,"updatedAt":now}).await.unwrap();
    app.database
        .collection::<Document>("_schema_migrations")
        .delete_one(doc! {"_id":"codegen"})
        .await
        .unwrap();
    admin_api::storage::schema::migrate_database(&app.database)
        .await
        .unwrap();
    assert_eq!(
        menus
            .find_one(doc! {"path":"/codegen"})
            .await
            .unwrap()
            .unwrap()
            .get_str("viewKey")
            .unwrap(),
        "dashboard"
    );
    app.cleanup().await;
}
