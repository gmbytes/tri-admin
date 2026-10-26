#[path = "support/mongo_faults.rs"]
mod mongo_faults;
#[path = "support/s3_faults.rs"]
mod s3_faults;
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
    server_with_uri(database, root, None)
}
fn server_with_uri(
    database: &mongodb::Database,
    root: &std::path::Path,
    uri: Option<&str>,
) -> (Server, String) {
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let log_path = root.join(format!("export-service-{}.log", uuid::Uuid::new_v4()));
    let log = std::fs::File::create(&log_path).unwrap();
    let mut command = command(database, root);
    if let Some(uri) = uri {
        command.env("TRI_MONGODB_URI", uri);
    }
    let child = command
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

#[tokio::test]
async fn killed_main_during_xlsx_generation_retires_partial_workspace_and_rebuilds_one_complete_result()
 {
    use calamine::Reader;
    let (app, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    let root = tempfile::tempdir().unwrap();
    let now = DateTime::now();
    for batch in 0..100 {
        let rows = (0..1000).map(|offset| {
            let index = batch * 1000 + offset;
            doc! {"_id":ObjectId::new(),"key":format!("kill.export.{index:06}"),"name":"Process death fixture","group":"kill","description":"A bounded real XLSX row used to observe durable progress before process termination.","valueType":"boolean","valueJson":"true","version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}
        }).collect::<Vec<_>>();
        app.database
            .collection::<Document>("parameters")
            .insert_many(rows)
            .await
            .unwrap();
    }
    let (code, result) = app.send("POST", "/api/v1/data-tools/exports", json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":"xlsx","maxRows":100000,"source":{"kind":"resource","resource":"parameters","columns":[{"field":"key","header":"key"},{"field":"name","header":"name"},{"field":"group","header":"group"},{"field":"description","header":"description"},{"field":"valueType","header":"type"},{"field":"value","header":"value"}],"search":"kill.export."}}), Some(token)).await;
    assert_eq!(code, StatusCode::ACCEPTED, "{result}");
    let id = ObjectId::parse_str(result["data"]["id"].as_str().unwrap()).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let (mut first, base) = server(&app.database, root.path());
    ready(&mut first, &client, &base).await;
    let before = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let row = app
                .database
                .collection::<Document>("data_exports")
                .find_one(doc! {"_id":id})
                .await
                .unwrap()
                .unwrap();
            assert_ne!(
                row.get_str("state").unwrap(),
                "completed",
                "missed the live generation boundary"
            );
            if row.get_str("state").unwrap() == "running"
                && row.get_i64("rowCount").unwrap() >= 1000
            {
                break row;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let candidate = app
        .database
        .collection::<Document>("data_export_attempts")
        .find_one(doc! {"_id":before.get_object_id("candidateId").unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(candidate.get_str("state").unwrap(), "generating");
    assert!(!candidate.get_bool("writeStarted").unwrap());
    let workspace = root
        .path()
        .join(".tri-export-work")
        .join(candidate.get_str("workspaceId").unwrap());
    assert!(workspace.join("artifact.part").is_file());
    assert!(matches!(first.0.try_wait(), Ok(None)));
    first.0.kill().unwrap();
    assert!(!first.0.wait().unwrap().success());
    drop(first);
    // Advance only the isolated fixture's expired lease after confirmed process death.
    app.database.collection::<Document>("data_exports").update_one(doc!{"_id":id},doc!{"$set":{"leaseExpiresAt":DateTime::from_millis(0),"nextRunAt":DateTime::from_millis(0)}}).await.unwrap();
    let (mut second, base) = server(&app.database, root.path());
    ready(&mut second, &client, &base).await;
    wait_job(&app, token, &id.to_hex(), "completed", true).await;
    let after = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert!(after.get_i64("generation").unwrap() > before.get_i64("generation").unwrap());
    assert_ne!(
        after.get_object_id("candidateId").unwrap(),
        candidate.get_object_id("_id").unwrap()
    );
    let retired = app
        .database
        .collection::<Document>("data_export_attempts")
        .find_one(doc! {"_id":candidate.get_object_id("_id").unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retired.get_str("state").unwrap(), "cleaned");
    assert!(retired.get_bool("workspaceCleaned").unwrap());
    assert!(!retired.get_bool("writeStarted").unwrap());
    assert!(!workspace.exists());
    let response = client
        .get(format!("{base}/api/v1/data-tools/exports/{id}/content"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let bytes = response.bytes().await.unwrap();
    let mut workbook: calamine::Xlsx<_> =
        calamine::open_workbook_from_rs(std::io::Cursor::new(bytes)).unwrap();
    let sheet = workbook.worksheet_range_at(0).unwrap().unwrap();
    assert_eq!(sheet.height(), 100001);
    assert_eq!(sheet.width(), 6);
    assert_eq!(
        sheet.get_value((1, 0)),
        Some(&calamine::Data::String("kill.export.000000".into()))
    );
    assert_eq!(
        sheet.get_value((100000, 0)),
        Some(&calamine::Data::String("kill.export.099999".into()))
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.complete","targetId":id})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("files")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    drop(second);
    app.cleanup().await;
}

#[tokio::test]
#[ignore = "requires isolated real S3; run npm run test:s3"]
async fn real_s3_main_killed_during_put_recovers_original_late_object_without_another_write() {
    use s3_faults::{Fault, S3FaultProxy};
    let definition: Value = serde_json::from_slice(
        &std::fs::read(
            std::env::var("TRI_S3_FIXTURE_FILE")
                .expect("run node tools/test-s3-content.mjs --exports-only"),
        )
        .unwrap(),
    )
    .unwrap();
    let proxy = S3FaultProxy::start(&definition, Fault::WriteAfterRelease).await;
    let (app, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    let root = tempfile::tempdir().unwrap();
    let (code, profile) = app.send("POST", "/api/v1/settings/profiles", json!({"name":"main-process-s3","code":"main-process-s3","enabled":true,"configuration":{"kind":"s3","endpoint":proxy.endpoint,"bucket":definition["bucket"],"region":definition["region"],"prefix":format!("process-{}",uuid::Uuid::new_v4()),"pathStyle":true},"secret":{"kind":"s3","accessKeyId":definition["accessKeyId"],"secretAccessKey":definition["secretAccessKey"]}}), Some(token)).await;
    assert_eq!(code, StatusCode::CREATED);
    let (_, settings) = app
        .send("GET", "/api/v1/settings/service", Value::Null, Some(token))
        .await;
    assert_eq!(
        app.send(
            "PATCH",
            "/api/v1/settings/service",
            json!({"version":settings["data"]["version"],"storageProfileId":profile["data"]["id"]}),
            Some(token)
        )
        .await
        .0,
        StatusCode::OK
    );
    let id = job(&app, token, "csv").await;
    let object_id = ObjectId::parse_str(&id).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let (mut first, base) = server(&app.database, root.path());
    ready(&mut first, &client, &base).await;
    proxy.wait_for_write().await;
    let before = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":object_id})
        .await
        .unwrap()
        .unwrap();
    let candidate = app
        .database
        .collection::<Document>("data_export_attempts")
        .find_one(doc! {"_id":before.get_object_id("candidateId").unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.get_str("state").unwrap(), "publishing");
    assert_eq!(candidate.get_str("state").unwrap(), "putting");
    assert!(candidate.get_bool("writeStarted").unwrap());
    assert!(!candidate.get_bool("blobKnown").unwrap());
    assert!(matches!(first.0.try_wait(), Ok(None)));
    first.0.kill().unwrap();
    assert!(!first.0.wait().unwrap().success());
    drop(first);
    app.database
        .collection::<Document>("parameters")
        .update_many(doc! {}, doc! {"$set":{"valueJson":"false"}})
        .await
        .unwrap();
    proxy.release_late_write().await;
    proxy.set_fault(Fault::None);
    app.database.collection::<Document>("data_exports").update_one(doc!{"_id":object_id},doc!{"$set":{"leaseExpiresAt":DateTime::from_millis(0),"nextRunAt":DateTime::from_millis(0)}}).await.unwrap();
    let (mut second, base) = server(&app.database, root.path());
    ready(&mut second, &client, &base).await;
    wait_job(&app, token, &id, "completed", true).await;
    let response = client
        .get(format!("{base}/api/v1/data-tools/exports/{id}/content"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let bytes = response.bytes().await.unwrap();
    assert_eq!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .trim_start_matches('\u{feff}'),
        "key,value\nservice.export.one,true\n"
    );
    let after = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":object_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.get_object_id("candidateId").unwrap(),
        before.get_object_id("candidateId").unwrap()
    );
    assert_eq!(
        after.get_i64("generation").unwrap(),
        before.get_i64("generation").unwrap()
    );
    assert_eq!(proxy.puts(), 1);
    assert!(proxy.reads() >= 2);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.complete","targetId":object_id})
            .await
            .unwrap(),
        1
    );
    drop(second);
    app.cleanup().await;
}

#[tokio::test]
async fn main_retries_a_confirmed_but_lost_completion_commit_reply_without_reexecuting_publication()
{
    let (app, auth) = fixture().await;
    let token = auth["accessToken"].as_str().unwrap();
    let root = tempfile::tempdir().unwrap();
    let id = job(&app, token, "csv").await;
    let object_id = ObjectId::parse_str(&id).unwrap();
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let environment = dotenvy::from_path_iter(project.join(".env"))
        .unwrap()
        .map(Result::unwrap)
        .collect::<std::collections::HashMap<_, _>>();
    let proxy = mongo_faults::MongoCommitFault::start(
        environment.get("TRI_MONGODB_URI").unwrap(),
        app.database.name(),
        object_id,
    )
    .await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let (mut first, base) = server_with_uri(&app.database, root.path(), Some(&proxy.uri));
    ready(&mut first, &client, &base).await;
    proxy.wait_for_lost_commit().await;
    wait_job(&app, token, &id, "completed", true).await;
    assert_eq!(proxy.dropped(), 1);
    assert!(
        proxy.commits() >= 2,
        "the same committed transaction must be confirmed again"
    );
    assert_eq!(
        proxy.repeated_body(),
        0,
        "the completion transaction body must not be repeated after commit success"
    );
    let before = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":object_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        app.database
            .collection::<Document>("data_export_attempts")
            .count_documents(doc! {"jobId":object_id})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.complete","targetId":object_id})
            .await
            .unwrap(),
        1
    );
    drop(first);
    app.database
        .collection::<Document>("parameters")
        .update_many(doc! {}, doc! {"$set":{"valueJson":"false"}})
        .await
        .unwrap();
    let (mut second, base) = server_with_uri(&app.database, root.path(), Some(&proxy.uri));
    ready(&mut second, &client, &base).await;
    wait_job(&app, token, &id, "completed", true).await;
    let response = client
        .get(format!("{base}/api/v1/data-tools/exports/{id}/content"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        String::from_utf8(response.bytes().await.unwrap().to_vec())
            .unwrap()
            .trim_start_matches('\u{feff}'),
        "key,value\nservice.export.one,true\n"
    );
    let after = app
        .database
        .collection::<Document>("data_exports")
        .find_one(doc! {"_id":object_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.get_object_id("candidateId").unwrap(),
        before.get_object_id("candidateId").unwrap()
    );
    assert_eq!(
        after.get_i64("generation").unwrap(),
        before.get_i64("generation").unwrap()
    );
    assert_eq!(proxy.dropped(), 1);
    assert_eq!(proxy.repeated_body(), 0);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.export.complete","targetId":object_id})
            .await
            .unwrap(),
        1
    );
    drop(second);
    app.cleanup().await;
}
