mod support;
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{Document, doc};
use serde_json::json;
use sha2::{Digest, Sha256};
use support::{PASSWORD, TestApp};

#[tokio::test]
async fn inspect_returns_only_bounded_headers_geometry_and_original_content_hash_without_writes() {
    use admin_api::modules::data_tools::formats::{self, Cell, Format, Table};
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let table = Table {
        headers: vec!["名称".into(), "标识".into(), "类型".into(), "值".into()],
        rows: vec![vec![
            Cell::Text("private payload".into()),
            Cell::Text("custom.key".into()),
            Cell::Text("boolean".into()),
            Cell::Boolean(true),
        ]],
    };
    for format in [Format::Csv, Format::Xlsx] {
        let bytes = formats::encode(format, &table).unwrap();
        let result=app.send("POST","/api/v1/data-tools/inspect",json!({"resource":"parameters","format":format,"contentBase64":STANDARD.encode(&bytes)}),Some(access)).await;
        assert_eq!(result.0, StatusCode::OK, "{}", result.1);
        assert_eq!(result.1["data"]["headers"], json!(table.headers));
        assert_eq!(result.1["data"]["rowCount"], 1);
        assert_eq!(
            result.1["data"]["sha256"],
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert_eq!(result.1["data"].as_object().unwrap().len(), 5);
        assert!(!result.1.to_string().contains("private payload"));
    }
    assert_eq!(
        app.database
            .collection::<Document>("data_imports")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    app.cleanup().await;
}

#[tokio::test]
async fn inspect_checks_current_resource_write_permissions_input_types_and_size() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let role=app.send("POST","/api/v1/roles",json!({"name":"inspect-reader","slug":"inspect-reader","permissions":["exports.write","parameters.read"],"dataScope":"self"}),Some(access)).await.1["data"].clone();
    app.send("POST","/api/v1/users",json!({"username":"inspect-reader","displayName":"Reader","password":PASSWORD,"roleIds":[role["id"]]}),Some(access)).await;
    let auth = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":"inspect-reader","password":PASSWORD}),
            None,
        )
        .await
        .1;
    let token = auth["data"]["accessToken"].as_str().unwrap();
    let body = json!({"resource":"parameters","format":"csv","contentBase64":STANDARD.encode("name,key\nA,a\n")});
    assert_eq!(
        app.send(
            "POST",
            "/api/v1/data-tools/inspect",
            body.clone(),
            Some(token)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let mut invalid = body.clone();
    invalid["sql"] = json!("arbitrary expression");
    assert_eq!(
        app.send("POST", "/api/v1/data-tools/inspect", invalid, Some(access))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for content in ["!".to_string(), STANDARD.encode("name,name\na,b\n")] {
        let mut invalid = body.clone();
        invalid["contentBase64"] = json!(content);
        assert_eq!(
            app.send("POST", "/api/v1/data-tools/inspect", invalid, Some(access))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut large = body;
    large["contentBase64"] = json!("A".repeat(8 * 1024 * 1024 + 4));
    assert_eq!(
        app.send("POST", "/api/v1/data-tools/inspect", large, Some(access))
            .await
            .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    app.cleanup().await;
}
