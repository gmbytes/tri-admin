mod support;
use axum::http::StatusCode;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::TestApp;
struct Server(std::process::Child, std::path::PathBuf);
impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
        if std::thread::panicking() {
            let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.superpowers/sdd/2026-10-24-tri-admin-greenfield")
                .join(format!("export-service-failure-{}.log", self.0.id()));
            let _ = std::fs::copy(&self.1, target);
        }
    }
}
fn command(database: &mongodb::Database, root: &std::path::Path) -> std::process::Command {
    assert!(database.name().starts_with("tri_admin_test_"));
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let environment = dotenvy::from_path_iter(project.join(".env"))
        .unwrap()
        .map(Result::unwrap)
        .collect::<std::collections::HashMap<_, _>>();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_admin-api"));
    command
        .current_dir(project)
        .env(
            "TRI_MONGODB_URI",
            environment.get("TRI_MONGODB_URI").unwrap(),
        )
        .env("TRI_MONGODB_DATABASE", database.name())
        .env("TRI_JWT_SIGNING_KEY", support::KEY)
        .env("TRI_SETTINGS_KEYS", format!("fixture:{}", "11".repeat(32)))
        .env("TRI_SETTINGS_ACTIVE_KEY", "fixture")
        .env("TRI_STORAGE_ROOT", root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}
fn server(database: &mongodb::Database, root: &std::path::Path) -> (Server, String) {
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let log_path = root.join(format!("export-service-{}.log", uuid::Uuid::new_v4()));
    let log = std::fs::File::create(&log_path).unwrap();
    let child = command(database, root)
        .arg("serve")
        .env("TRI_ADMIN_BIND", address.to_string())
        .stdout(std::process::Stdio::from(log.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(log))
        .spawn()
        .unwrap();
    (Server(child, log_path), format!("http://{address}"))
}
async fn ready(server: &mut Server, client: &reqwest::Client, base: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            assert!(matches!(server.0.try_wait(), Ok(None)));
            if client
                .get(format!("{base}/health/ready"))
                .send()
                .await
                .is_ok_and(|response| response.status().as_u16() == 200)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
}
async fn job(app: &TestApp, token: &str, format: &str) -> String {
    let result=app.send("POST","/api/v1/data-tools/exports",json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":format,"maxRows":10,"source":{"kind":"resource","resource":"parameters","columns":[{"field":"key","header":"key"},{"field":"value","header":"value"}],"search":"service.export."}}),Some(token)).await;
    assert_eq!(result.0, StatusCode::ACCEPTED, "{}", result.1);
    result.1["data"]["id"].as_str().unwrap().into()
}
async fn wait_job(app: &TestApp, token: &str, id: &str, expected: &str, cleaned: bool) {
    let mut last = Value::Null;
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(25), async {
        loop {
            let result = app
                .send(
                    "GET",
                    &format!("/api/v1/data-tools/exports/{id}"),
                    Value::Null,
                    Some(token),
                )
                .await
                .1["data"]
                .clone();
            last = result.clone();
            if result["state"] == expected && (!cleaned || result["cleanupPending"] == false) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await;
    if outcome.is_err() {
        let statistics = app
            .database
            .client()
            .database("admin")
            .run_command(doc! {"serverStatus":1,"transactions":1})
            .await
            .ok();
        let counts = statistics
            .as_ref()
            .and_then(|value| value.get_document("transactions").ok())
            .map(|value| {
                [
                    "currentOpen",
                    "currentActive",
                    "currentInactive",
                    "totalAborted",
                ]
                .map(|key| (key, value.get(key).cloned()))
            });
        panic!("expected {expected}, last public job={last}; transaction counters={counts:?}");
    }
}
async fn fixture() -> (TestApp, Value) {
    let app = TestApp::new().await;
    let auth = app.login().await;
    assert_eq!(app.send("POST","/api/v1/parameters",json!({"name":"Service fixture","key":"service.export.one","valueType":"boolean","value":true}),Some(auth["accessToken"].as_str().unwrap())).await.0,StatusCode::CREATED);
    (app, auth)
}

#[tokio::test]
async fn real_main_processes_restart_queue_and_periodic_expiry_and_exposes_a_maintenance_cli() {
    let (app, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    let root = tempfile::tempdir().unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let first_job = job(&app, token, "csv").await;
    let (mut first, base) = server(&app.database, root.path());
    ready(&mut first, &client, &base).await;
    wait_job(&app, token, &first_job, "completed", true).await;
    drop(first);
    let second_job = job(&app, token, "xlsx").await;
    let (mut second, base) = server(&app.database, root.path());
    ready(&mut second, &client, &base).await;
    wait_job(&app, token, &second_job, "completed", true).await;
    assert_eq!(
        client
            .get(format!(
                "{base}/api/v1/data-tools/exports/{second_job}/content"
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    for id in [&first_job, &second_job] {
        app.database
            .collection::<Document>("data_exports")
            .update_one(
                doc! {"_id":ObjectId::parse_str(id).unwrap()},
                doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
        app.database
            .collection::<Document>("data_export_attempts")
            .update_many(
                doc! {"jobId":ObjectId::parse_str(id).unwrap()},
                doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
    }
    wait_job(&app, token, &first_job, "expired", true).await;
    wait_job(&app, token, &second_job, "expired", true).await;
    drop(second);
    let output = command(&app.database, root.path())
        .arg("maintain-exports")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["retired"], 0);
    assert_eq!(report["pending"], 0);
    app.cleanup().await;
}

#[tokio::test]
async fn real_main_reconciles_the_original_written_candidate_after_process_death() {
    let (app, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    let root = tempfile::tempdir().unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.export.complete"}},"validationLevel":"strict"}).await.unwrap();
    let id = job(&app, token, "csv").await;
    let (mut first, base) = server(&app.database, root.path());
    ready(&mut first, &client, &base).await;
    wait_job(&app, token, &id, "unknown", false).await;
    // Kill at a durably parked publication boundary. Cleanup-vs-execution lease
    // conflicts and stale cleanup leases are exercised deterministically in the
    // worker tests, rather than relying on a timing race in this process test.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let attempt = app
                .database
                .collection::<Document>("data_export_attempts")
                .find_one(doc! {"jobId":ObjectId::parse_str(&id).unwrap()})
                .await
                .unwrap()
                .unwrap();
            if attempt.get_bool("workspaceCleaned").unwrap()
                && matches!(attempt.get("cleanupToken"), Some(mongodb::bson::Bson::Null))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(first);
    let before = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":ObjectId::parse_str(&id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    app.database
        .collection::<Document>("parameters")
        .update_many(doc! {}, doc! {"$set":{"valueJson":"false"}})
        .await
        .unwrap();
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    app.database
        .collection::<Document>("data_exports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(&id).unwrap()},
            doc! {"$set":{"nextRunAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let (mut second, base) = server(&app.database, root.path());
    ready(&mut second, &client, &base).await;
    wait_job(&app, token, &id, "completed", true).await;
    let bytes = client
        .get(format!("{base}/api/v1/data-tools/exports/{id}/content"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(String::from_utf8(bytes.to_vec()).unwrap().contains("true"));
    let after = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":ObjectId::parse_str(&id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before.get_object_id("candidateId").unwrap(),
        after.get_object_id("candidateId").unwrap()
    );
    assert_eq!(
        before.get_i64("generation").unwrap(),
        after.get_i64("generation").unwrap()
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.complete"})
            .await
            .unwrap(),
        1
    );
    drop(second);
    app.cleanup().await;
}
