mod support;
use axum::http::StatusCode;
use serde_json::{Value, json};
use support::TestApp;
#[tokio::test]
#[ignore = "requires the isolated Docker fixture; run npm run test:s3"]
async fn real_s3_service_accepts_the_signature_and_rejects_wrong_secret() {
    let path = std::env::var("TRI_S3_FIXTURE_FILE").expect("run npm run test:s3");
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    for (code, secret, success) in [
        (
            "s3-correct",
            fixture["secretAccessKey"].as_str().unwrap().to_owned(),
            true,
        ),
        (
            "s3-wrong",
            format!("{}-wrong", fixture["secretAccessKey"].as_str().unwrap()),
            false,
        ),
    ] {
        let(status,profile)=app.send("POST","/api/v1/settings/profiles",json!({"name":code,"code":code,"enabled":true,"configuration":{"kind":"s3","endpoint":fixture["endpoint"],"bucket":fixture["bucket"],"region":fixture["region"],"prefix":"","pathStyle":true},"secret":{"kind":"s3","accessKeyId":fixture["accessKeyId"],"secretAccessKey":secret}}),Some(token)).await;
        assert_eq!(status, StatusCode::CREATED, "{profile}");
        let (status, result) = app
            .send(
                "POST",
                &format!(
                    "/api/v1/settings/profiles/{}/probe",
                    profile["data"]["id"].as_str().unwrap()
                ),
                json!({"version":1}),
                Some(token),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["data"]["success"], success, "{result}");
        assert_eq!(result["data"]["scope"], "s3BucketHead");
        if !success {
            assert_eq!(result["data"]["errorCode"], "UPSTREAM_AUTH_REJECTED");
        }
        assert!(
            !result
                .to_string()
                .contains(fixture["secretAccessKey"].as_str().unwrap())
        );
    }
    app.cleanup().await;
}
