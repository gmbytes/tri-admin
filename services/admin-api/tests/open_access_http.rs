mod support;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::TestApp;
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[tokio::test]
async fn real_http_token_calls_enforce_scope_and_socket_ip_and_main_runs_usage_maintenance() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = ObjectId::parse_str(auth["user"]["id"].as_str().unwrap()).unwrap();
    app.database.collection::<Document>("api_call_logs").insert_one(doc!{"_id":ObjectId::new(),"ownerId":owner,"createdAt":DateTime::from_millis(DateTime::now().timestamp_millis()-31*86400000),"expireAt":DateTime::from_millis(DateTime::now().timestamp_millis()+86400000)}).await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let environment = dotenvy::from_path_iter(project.join(".env"))
        .unwrap()
        .map(Result::unwrap)
        .collect::<std::collections::HashMap<_, _>>();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    assert!(app.database.name().starts_with("tri_admin_test_"));
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_admin-api"));
    command
        .current_dir(&project)
        .arg("serve")
        .env(
            "TRI_MONGODB_URI",
            environment.get("TRI_MONGODB_URI").unwrap(),
        )
        .env("TRI_MONGODB_DATABASE", app.database.name())
        .env("TRI_JWT_SIGNING_KEY", support::KEY)
        .env(
            "TRI_OPEN_JWT_SIGNING_KEY",
            "separate-open-http-fixture-signing-key-2026",
        )
        .env("TRI_SETTINGS_KEYS", format!("fixture:{}", "11".repeat(32)))
        .env("TRI_SETTINGS_ACTIVE_KEY", "fixture")
        .env("TRI_STORAGE_ROOT", root.path())
        .env("TRI_ADMIN_BIND", address.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let base = format!("http://{address}");
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            assert!(matches!(child.0.try_wait(), Ok(None)));
            if client
                .get(format!("{base}/health/ready"))
                .send()
                .await
                .is_ok_and(|r| r.status().as_u16() == 200)
                && app
                    .database
                    .collection::<Document>("api_call_logs")
                    .count_documents(doc! {})
                    .await
                    .unwrap()
                    == 0
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let response = client
        .post(format!("{base}/api/v1/auth/login"))
        .header("content-type", "application/json")
        .body(json!({"username":"admin","password":support::PASSWORD}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let auth: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let session = auth["data"]["accessToken"].as_str().unwrap();
    let response=client.post(format!("{base}/api/v1/api-tokens")).bearer_auth(session).header("content-type","application/json").body(json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Real network","scopes":["parameters.read"],"allowedIps":["127.0.0.0/8"]}).to_string()).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 201);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let issued: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let token = issued["data"]["secret"].as_str().unwrap();
    let response = client
        .get(format!("{base}/api/v1/parameters"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(
        client
            .post(format!("{base}/api/v1/parameters"))
            .bearer_auth(token)
            .header("content-type", "application/json")
            .body(
                json!({"name":"Denied","key":"denied","valueType":"boolean","value":true})
                    .to_string()
            )
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    let response=client.post(format!("{base}/api/v1/api-tokens")).bearer_auth(session).header("content-type","application/json").body(json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"Wrong origin","scopes":["parameters.read"],"allowedIps":["192.0.2.0/24"]}).to_string()).send().await.unwrap();
    let blocked: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(
        client
            .get(format!("{base}/api/v1/parameters"))
            .bearer_auth(blocked["data"]["secret"].as_str().unwrap())
            .header("x-forwarded-for", "192.0.2.7")
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    let response = client
        .get(format!("{base}/api/v1/api-tokens/calls"))
        .bearer_auth(session)
        .send()
        .await
        .unwrap();
    let page: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(page["meta"]["total"], 3);
    assert!(
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["ip"] == "127.0.0.1")
    );
    assert!(!page.to_string().contains(token));
    let response=client.post(format!("{base}/api/v1/applications")).bearer_auth(session).header("content-type","application/json").body(json!({"requestId":uuid::Uuid::new_v4().to_string(),"name":"HTTP application","code":"http-application","allowedScopes":["open.profile.read","open.profile.write","open.users.read","open.users.write"]}).to_string()).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 201);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let created: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let application = created["data"]["application"]["id"].as_str().unwrap();
    let response = client
        .post(format!("{base}/api/v1/applications/{application}/users"))
        .bearer_auth(session)
        .header("content-type", "application/json")
        .body(
            json!({"username":"http.user","displayName":"HTTP user","password":support::PASSWORD})
                .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 201);
    let account: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(account["data"]["applicationId"], application);
    assert!(!account.to_string().contains("passwordHash"));
    let response=client.post(format!("{base}/open/v1/auth/login")).header("content-type","application/json").body(json!({"applicationId":application,"username":"http.user","password":support::PASSWORD}).to_string()).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let open: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let access = open["data"]["accessToken"].as_str().unwrap();
    assert_eq!(
        client
            .get(format!("{base}/open/v1/me"))
            .bearer_auth(access)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    assert_eq!(
        client
            .get(format!("{base}/api/v1/auth/me"))
            .bearer_auth(access)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    let response = client
        .post(format!("{base}/open/v1/auth/refresh"))
        .header("content-type", "application/json")
        .body(
            json!({"applicationId":application,"refreshToken":open["data"]["refreshToken"]})
                .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let response = client
        .post(format!("{base}/open/v1/auth/application"))
        .header("content-type", "application/json")
        .body(json!({"applicationId":application,"secret":created["data"]["secret"]}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let service: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let service = service["data"]["accessToken"].as_str().unwrap();
    let response=client.post(format!("{base}/open/v1/users")).bearer_auth(service).header("content-type","application/json").body(json!({"username":"wire.provisioned","displayName":"Service wire user","password":support::PASSWORD,"scopes":["open.profile.read"]}).to_string()).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 201);
    let provisioned: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let subject = provisioned["data"]["id"].as_str().unwrap();
    assert_eq!(provisioned["data"]["applicationId"], application);
    assert_eq!(
        client
            .get(format!("{base}/open/v1/users"))
            .bearer_auth(service)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    assert_eq!(
        client
            .get(format!("{base}/open/v1/users"))
            .bearer_auth(access)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    assert_eq!(
        client
            .post(format!("{base}/open/v1/users/{subject}/password"))
            .bearer_auth(service)
            .header("content-type", "application/json")
            .body(json!({"version":1,"password":"WireResetPassword-2026"}).to_string())
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        204
    );
    let response = client
        .get(format!(
            "{base}/api/v1/applications/{application}/calls?principalKind=application"
        ))
        .bearer_auth(session)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let history: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(history["meta"]["total"], 3);
    assert!(!history.to_string().contains(service));
    drop(child);
    let mut maintenance = std::process::Command::new(env!("CARGO_BIN_EXE_admin-api"));
    maintenance
        .current_dir(project)
        .arg("maintain-access")
        .env(
            "TRI_MONGODB_URI",
            environment.get("TRI_MONGODB_URI").unwrap(),
        )
        .env("TRI_MONGODB_DATABASE", app.database.name())
        .env("TRI_JWT_SIGNING_KEY", support::KEY);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        maintenance.creation_flags(0x08000000);
    }
    let output = maintenance.output().unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report, json!({"calls":0,"buckets":0,"attempts":0}));
    app.cleanup().await;
}
