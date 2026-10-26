mod support;
use axum::http::StatusCode;
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use support::{PASSWORD, TestApp};

async fn catalog(app: &TestApp, token: &str) -> Value {
    let (status, body) = app
        .send("GET", "/api/v1/api-catalog", Value::Null, Some(token))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}
async fn identity(app: &TestApp, owner: &str, codes: Value) -> (String, ObjectId) {
    let (status,role)=app.send("POST","/api/v1/roles",json!({"name":"catalog-actor","slug":"catalog-actor","permissions":codes,"dataScope":"all","dataScopeRoleIds":[]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{role}");
    let role_id = role["data"]["id"].as_str().unwrap();
    let (status,body)=app.send("POST","/api/v1/users",json!({"username":"catalog-actor","displayName":"目录验证","password":PASSWORD,"roleIds":[role_id]}),Some(owner)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, body) = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"catalog-actor","password":PASSWORD}),
            None,
        )
        .await;
    (
        body["data"]["accessToken"].as_str().unwrap().into(),
        ObjectId::parse_str(role_id).unwrap(),
    )
}
fn path(endpoint: &Value) -> String {
    endpoint["path"]
        .as_str()
        .unwrap()
        .split('/')
        .map(|part| {
            if part.starts_with('{') {
                "000000000000000000000000"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[tokio::test]
async fn catalog_has_stable_unique_operations_and_a_verifiable_checksum() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let data = catalog(&app, token).await;
    assert_eq!(data["schemaVersion"], 2);
    let entries = data["endpoints"].as_array().unwrap();
    assert!(entries.len() > 30);
    let mut ids = HashSet::new();
    let mut routes = HashSet::new();
    let mut previous = "";
    let (_, permissions) = app
        .send("GET", "/api/v1/permissions", Value::Null, Some(token))
        .await;
    let codes = permissions["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["code"].as_str().unwrap())
        .collect::<HashSet<_>>();
    for entry in entries {
        let id = entry["operationId"].as_str().unwrap();
        assert!(ids.insert(id));
        assert!(id > previous);
        previous = id;
        assert!(routes.insert((
            entry["method"].as_str().unwrap(),
            entry["path"].as_str().unwrap()
        )));
        assert!(!entry["label"].as_str().unwrap().is_empty());
        assert!(!entry["group"].as_str().unwrap().is_empty());
        if entry["access"] == "permission" {
            let required = entry["requiredPermissions"].as_array().unwrap();
            assert!(!required.is_empty());
            let mut previous = "";
            for code in required {
                let code = code.as_str().unwrap();
                assert!(codes.contains(code));
                assert!(code > previous);
                previous = code;
            }
        } else {
            assert_eq!(entry["requiredPermissions"], json!([]));
        }
    }
    let checksum = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&data["endpoints"]).unwrap())
    );
    assert_eq!(data["checksum"], checksum);
    assert_eq!(catalog(&app, token).await["checksum"], checksum);
    for required in [
        "apiCatalog.list",
        "parameters.get",
        "roles.assignable",
        "dictionaries.items.update",
        "auth.refresh",
        "health.ready",
    ] {
        assert!(ids.contains(required), "{required}");
    }
    assert_eq!(
        app.send(
            "GET",
            "/api/v1/api-catalog?unknown=value",
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
async fn every_catalog_permission_matches_the_real_route_denial_gate() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let data = catalog(&app, owner).await;
    let (_, permissions) = app
        .send("GET", "/api/v1/permissions", Value::Null, Some(owner))
        .await;
    let all = permissions["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["code"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let (token, role_id) = identity(&app, owner, json!(all)).await;
    let mut checked = 0;
    for entry in data["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["access"] == "permission")
    {
        let required = entry["requiredPermissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        let payload = if entry["operationId"] == "users.create" {
            json!({"username":"catalog-created","displayName":"Created","password":PASSWORD,"roleIds":[role_id.to_hex()]})
        } else {
            json!({})
        };
        for code in &required {
            let codes = all
                .iter()
                .filter(|value| value.as_str() != *code)
                .cloned()
                .collect::<Vec<_>>();
            app.database
                .collection::<Document>("roles")
                .update_one(doc! {"_id":role_id}, doc! {"$set":{"permissions":codes}})
                .await
                .unwrap();
            let (status, body) = app
                .send(
                    entry["method"].as_str().unwrap(),
                    &path(entry),
                    payload.clone(),
                    Some(&token),
                )
                .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "operation={}, body={body}",
                entry["operationId"]
            );
            checked += 1;
        }
        app.database
            .collection::<Document>("roles")
            .update_one(doc! {"_id":role_id}, doc! {"$set":{"permissions":required}})
            .await
            .unwrap();
        let (status, body) = app
            .send(
                entry["method"].as_str().unwrap(),
                &path(entry),
                payload,
                Some(&token),
            )
            .await;
        assert!(
            !matches!(
                status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::METHOD_NOT_ALLOWED
            ),
            "minimum permissions insufficient: {} {status} {body}",
            entry["operationId"]
        );
    }
    assert!(checked > 30);
    println!("catalog permission gates checked: {checked}");
    app.cleanup().await;
}

#[tokio::test]
async fn catalog_access_classes_match_actual_unauthenticated_behavior() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let data = catalog(&app, owner).await;
    for entry in data["endpoints"].as_array().unwrap() {
        let (status, body) = app
            .send(
                entry["method"].as_str().unwrap(),
                &path(entry),
                json!({}),
                None,
            )
            .await;
        match entry["access"].as_str().unwrap() {
            "permission" | "authenticated" | "openApplication" | "openUser" => assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{} {body}",
                entry["operationId"]
            ),
            "public" | "refreshCredential" | "openRefreshCredential" => assert!(
                !matches!(
                    status,
                    StatusCode::FORBIDDEN
                        | StatusCode::NOT_FOUND
                        | StatusCode::METHOD_NOT_ALLOWED
                        | StatusCode::UNAUTHORIZED
                ),
                "{} {status} {body}",
                entry["operationId"]
            ),
            other => panic!("unknown access {other}"),
        }
    }
    let (token, _) = identity(&app, owner, json!(["users.read"])).await;
    assert_eq!(
        app.send("GET", "/api/v1/api-catalog", Value::Null, Some(&token))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    app.cleanup().await;
}
