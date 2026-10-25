mod support;
use axum::http::StatusCode;
use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use support::TestApp;

async fn job(app: &TestApp, access: &str) -> (Value, String) {
    let input = json!({"requestId":uuid::Uuid::new_v4().to_string(),"resource":"parameters","format":"csv","contentBase64":STANDARD.encode("name,key,valueType,value\nPrivate,retained.first,string,PRIVATE_SOURCE_SENTINEL\nPrivate,retained.second,string,PRIVATE_SOURCE_SENTINEL\n")});
    let result = app
        .send(
            "POST",
            "/api/v1/data-tools/imports",
            input.clone(),
            Some(access),
        )
        .await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    (input, result.1["data"]["id"].as_str().unwrap().into())
}
#[tokio::test]
async fn cleanup_removes_payload_atomically_preserves_receipts_and_never_recreates_a_deleted_target()
 {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (input, id) = job(&app, access).await;
    let path = format!("/api/v1/data-tools/imports/{id}/apply");
    let applied = app
        .send("POST", &path, json!({"rows":[0]}), Some(access))
        .await;
    assert_eq!(applied.0, StatusCode::OK, "{}", applied.1);
    let target = applied.1["data"]["rows"][0]["targetId"].as_str().unwrap();
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(&id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let report = admin_api::modules::data_tools::retention::maintenance_once(&app.database)
        .await
        .unwrap();
    assert_eq!(report.redacted, 1);
    let raw = app
        .database
        .collection::<Document>("data_imports")
        .find_one(doc! {"_id":ObjectId::parse_str(&id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert!(!raw.to_string().contains("PRIVATE_SOURCE_SENTINEL"));
    assert!(!raw.to_string().contains("retained.first"));
    assert_eq!(raw.get_str("payloadState").unwrap(), "redacted");
    let result = app
        .send(
            "GET",
            &format!("/api/v1/data-tools/imports/{id}"),
            Value::Null,
            Some(access),
        )
        .await
        .1;
    assert_eq!(result["data"]["payloadState"], "redacted");
    assert_eq!(result["data"]["status"], "partial");
    assert_eq!(result["data"]["appliedCount"], 1);
    assert_eq!(result["data"]["expiredCount"], 1);
    assert_eq!(result["data"]["rows"][0]["targetId"], target);
    assert!(result["data"]["rows"][0]["values"].is_null());
    assert_eq!(
        app.send(
            "DELETE",
            &format!("/api/v1/parameters/{target}?version=1"),
            Value::Null,
            Some(access)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.send("POST", &path, json!({"rows":[0,1]}), Some(access))
            .await
            .0,
        StatusCode::OK
    );
    let replay = app
        .send(
            "POST",
            "/api/v1/data-tools/imports",
            input.clone(),
            Some(access),
        )
        .await;
    assert_eq!(replay.1["data"]["id"], id);
    assert_eq!(replay.1["data"]["payloadState"], "redacted");
    let mut changed = input;
    changed["contentBase64"] =
        json!(STANDARD.encode("name,key,valueType,value\nChanged,other.key,string,value\n"));
    assert_eq!(
        app.send("POST", "/api/v1/data-tools/imports", changed, Some(access))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {"deletedAt":null})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        admin_api::modules::data_tools::retention::maintenance_once(&app.database)
            .await
            .unwrap()
            .redacted,
        0
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.import.redact"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
#[tokio::test]
async fn cleanup_is_bounded_skips_unexpired_content_and_concurrent_passes_do_not_duplicate_audit() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, id) = job(&app, access).await;
    let template = app
        .database
        .collection::<Document>("data_imports")
        .find_one(doc! {"_id":ObjectId::parse_str(&id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    let records = (0..101)
        .map(|_| {
            let mut row = template.clone();
            row.insert("_id", ObjectId::new());
            row.insert("requestId", uuid::Uuid::new_v4().to_string());
            row.insert("expiresAt", DateTime::from_millis(0));
            row
        })
        .collect::<Vec<_>>();
    app.database
        .collection::<Document>("data_imports")
        .insert_many(records)
        .await
        .unwrap();
    let first = admin_api::modules::data_tools::retention::maintenance_once(&app.database)
        .await
        .unwrap();
    assert_eq!(first.scanned, 100);
    assert_eq!(first.redacted, 100);
    let (a, b) = tokio::join!(
        admin_api::modules::data_tools::retention::maintenance_once(&app.database),
        admin_api::modules::data_tools::retention::maintenance_once(&app.database)
    );
    assert_eq!(a.unwrap().redacted + b.unwrap().redacted, 1);
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.import.redact"})
            .await
            .unwrap(),
        101
    );
    assert_eq!(
        app.database
            .collection::<Document>("data_imports")
            .find_one(doc! {"_id":ObjectId::parse_str(&id).unwrap()})
            .await
            .unwrap()
            .unwrap()
            .get_str("payloadState")
            .unwrap(),
        "full"
    );
    app.cleanup().await;
}
#[tokio::test]
async fn failed_cleanup_audit_rolls_back_payload_removal_and_can_be_retried() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, id) = job(&app, access).await;
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(&id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    app.database.run_command(doc!{"collMod":"audit_events","validator":{"action":{"$ne":"dataTools.import.redact"}},"validationLevel":"strict"}).await.unwrap();
    assert!(
        admin_api::modules::data_tools::retention::maintenance_once(&app.database)
            .await
            .is_err()
    );
    let raw = app
        .database
        .collection::<Document>("data_imports")
        .find_one(doc! {"_id":ObjectId::parse_str(&id).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw.get_str("payloadState").unwrap(), "full");
    assert!(raw.to_string().contains("PRIVATE_SOURCE_SENTINEL"));
    app.database
        .run_command(doc! {"collMod":"audit_events","validator":{}})
        .await
        .unwrap();
    assert_eq!(
        admin_api::modules::data_tools::retention::maintenance_once(&app.database)
            .await
            .unwrap()
            .redacted,
        1
    );
    app.cleanup().await;
}

struct Server(std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn server(database: &mongodb::Database, root: &std::path::Path) -> (Server, String) {
    assert!(database.name().starts_with("tri_admin_test_"));
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let env = dotenvy::from_path_iter(project.join(".env"))
        .unwrap()
        .map(Result::unwrap)
        .collect::<std::collections::HashMap<_, _>>();
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let log = std::fs::File::create(root.join(format!("retention-{}.log", uuid::Uuid::new_v4())))
        .unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_admin-api"));
    command
        .arg("serve")
        .current_dir(project)
        .env("TRI_MONGODB_URI", env.get("TRI_MONGODB_URI").unwrap())
        .env("TRI_MONGODB_DATABASE", database.name())
        .env("TRI_ADMIN_BIND", address.to_string())
        .env("TRI_JWT_SIGNING_KEY", support::KEY)
        .env("TRI_SETTINGS_KEYS", format!("fixture:{}", "11".repeat(32)))
        .env("TRI_SETTINGS_ACTIVE_KEY", "fixture")
        .env("TRI_STORAGE_ROOT", root)
        .stdout(std::process::Stdio::from(log.try_clone().unwrap()))
        .stderr(std::process::Stdio::from(log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    (
        Server(command.spawn().unwrap()),
        format!("http://{address}"),
    )
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
            tokio::time::sleep(std::time::Duration::from_millis(50)).await
        }
    })
    .await
    .unwrap();
}
async fn redacted(app: &TestApp, id: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            if app
                .database
                .collection::<Document>("data_imports")
                .find_one(doc! {"_id":ObjectId::parse_str(id).unwrap()})
                .await
                .unwrap()
                .unwrap()
                .get_str("payloadState")
                .unwrap()
                == "redacted"
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await
        }
    })
    .await
    .expect("owned main service did not redact the expired payload");
}
#[tokio::test]
async fn real_main_recovers_retention_after_restart_and_runs_periodic_cleanup() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, id) = job(&app, access).await;
    let root = tempfile::tempdir().unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    let (mut first, base) = server(&app.database, root.path());
    ready(&mut first, &client, &base).await;
    drop(first);
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(&id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let (mut second, base) = server(&app.database, root.path());
    ready(&mut second, &client, &base).await;
    redacted(&app, &id).await;
    let response = client
        .get(format!("{base}/api/v1/data-tools/imports/{id}"))
        .bearer_auth(access)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let result: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(result["data"]["payloadState"], "redacted");
    assert!(result["data"]["rows"][0]["values"].is_null());
    let (_, next) = job(&app, access).await;
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(&next).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    redacted(&app, &next).await;
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"dataTools.import.redact"})
            .await
            .unwrap(),
        2
    );
    drop(second);
    app.cleanup().await;
}

#[tokio::test]
async fn expired_apply_racing_cleanup_preserves_committed_rows_and_counter_truth() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let access = auth["accessToken"].as_str().unwrap();
    let (_, id) = job(&app, access).await;
    let path = format!("/api/v1/data-tools/imports/{id}/apply");
    assert_eq!(
        app.send("POST", &path, json!({"rows":[0]}), Some(access))
            .await
            .0,
        StatusCode::OK
    );
    app.database
        .collection::<Document>("data_imports")
        .update_one(
            doc! {"_id":ObjectId::parse_str(&id).unwrap()},
            doc! {"$set":{"expiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let (applied, cleaned) = tokio::join!(
        app.send("POST", &path, json!({"rows":[0,1]}), Some(access)),
        admin_api::modules::data_tools::retention::maintenance_once(&app.database)
    );
    assert!(matches!(applied.0, StatusCode::OK | StatusCode::CONFLICT));
    assert_eq!(cleaned.unwrap().redacted, 1);
    let result = app
        .send(
            "GET",
            &format!("/api/v1/data-tools/imports/{id}"),
            Value::Null,
            Some(access),
        )
        .await
        .1;
    assert_eq!(result["data"]["readyCount"], 0);
    assert_eq!(result["data"]["appliedCount"], 1);
    assert_eq!(result["data"]["errorCount"], 1);
    assert_eq!(result["data"]["expiredCount"], 1);
    assert_eq!(
        app.database
            .collection::<Document>("parameters")
            .count_documents(doc! {"deletedAt":null})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        app.database
            .collection::<Document>("audit_events")
            .count_documents(doc! {"action":"parameters.create"})
            .await
            .unwrap(),
        1
    );
    app.cleanup().await;
}
