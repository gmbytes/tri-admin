mod support;
use admin_api::{
    common::error::AppError,
    modules::data_tools::export_jobs::{authorization, models::JobRecord},
};
use axum::http::StatusCode;
use mongodb::bson::{DateTime, doc, oid::ObjectId};
use serde_json::json;
use support::{PASSWORD, TestApp};
async fn role(app: &TestApp, owner: &str, name: &str, scope: &str, ids: Vec<String>) -> String {
    let result=app.send("POST","/api/v1/roles",json!({"name":name,"slug":name,"permissions":["exports.read","exports.write","users.read"],"dataScope":scope,"dataScopeRoleIds":ids}),Some(owner)).await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    result.1["data"]["id"].as_str().unwrap().into()
}
async fn user(app: &TestApp, owner: &str, name: &str, role: &str) -> (ObjectId, String) {
    let result = app
        .send(
            "POST",
            "/api/v1/users",
            json!({"username":name,"displayName":name,"password":PASSWORD,"roleIds":[role]}),
            Some(owner),
        )
        .await;
    assert_eq!(result.0, StatusCode::CREATED, "{}", result.1);
    let id = ObjectId::parse_str(result.1["data"]["id"].as_str().unwrap()).unwrap();
    let login = app
        .send(
            "POST",
            "/api/v1/auth/login",
            json!({"username":name,"password":PASSWORD}),
            None,
        )
        .await;
    assert_eq!(login.0, StatusCode::OK);
    (id, login.1["data"]["accessToken"].as_str().unwrap().into())
}
async fn claimed(app: &TestApp, token: &str) -> JobRecord {
    let result=app.send("POST","/api/v1/data-tools/exports",json!({"requestId":uuid::Uuid::new_v4().to_string(),"format":"csv","source":{"kind":"resource","resource":"users","columns":[{"field":"username","header":"username"}]}}),Some(token)).await;
    assert_eq!(result.0, StatusCode::ACCEPTED, "{}", result.1);
    let id = ObjectId::parse_str(result.1["data"]["id"].as_str().unwrap()).unwrap();
    app.database.collection::<mongodb::bson::Document>("data_exports").update_one(doc!{"_id":id},doc!{"$set":{"state":"running","generation":1i64,"leaseToken":"fixture-lease","leaseExpiresAt":DateTime::from_millis(DateTime::now().timestamp_millis()+60000)}}).await.unwrap();
    reload(app, id).await
}
async fn reload(app: &TestApp, id: ObjectId) -> JobRecord {
    app.database
        .collection::<JobRecord>("data_exports")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn execution_identity_uses_live_permissions_and_active_user_state() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let role = role(&app, owner, "execution-operator", "all", vec![]).await;
    let (id, _) = user(&app, owner, "execution-operator", &role).await;
    assert!(
        authorization::execution_actor(&app.database, id, "users")
            .await
            .is_ok()
    );
    assert_eq!(
        app.send(
            "PATCH",
            &format!("/api/v1/roles/{role}"),
            json!({"version":1,"permissions":["exports.read","users.read"]}),
            Some(owner)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(matches!(
        authorization::execution_actor(&app.database, id, "users").await,
        Err(AppError::Forbidden)
    ));
    app.send(
        "PATCH",
        &format!("/api/v1/roles/{role}"),
        json!({"version":2,"permissions":["exports.read","exports.write","users.read"]}),
        Some(owner),
    )
    .await;
    app.send(
        "PATCH",
        &format!("/api/v1/users/{id}"),
        json!({"version":1,"status":"disabled"}),
        Some(owner),
    )
    .await;
    assert!(matches!(
        authorization::execution_actor(&app.database, id, "users").await,
        Err(AppError::Forbidden)
    ));
    app.cleanup().await;
}

#[tokio::test]
async fn manifest_batches_are_fenced_idempotent_and_rechecked_after_membership_or_scope_changes() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let team = role(&app, owner, "manifest-team", "self", vec![]).await;
    let other = role(&app, owner, "manifest-other", "self", vec![]).await;
    let (first, _) = user(&app, owner, "manifest-first", &team).await;
    let (second, _) = user(&app, owner, "manifest-second", &team).await;
    let operator_role = role(
        &app,
        owner,
        "manifest-operator",
        "roles",
        vec![team.clone()],
    )
    .await;
    let (operator_id, token) = user(&app, owner, "manifest-operator", &operator_role).await;
    let job = claimed(&app, &token).await;
    let id = job.meta.id;
    assert!(
        authorization::record_manifest_batch(
            &app.database,
            id,
            1,
            "fixture-lease",
            0,
            vec![first, second]
        )
        .await
        .unwrap()
    );
    assert!(
        !authorization::record_manifest_batch(
            &app.database,
            id,
            1,
            "fixture-lease",
            0,
            vec![first, second]
        )
        .await
        .unwrap()
    );
    assert!(matches!(
        authorization::record_manifest_batch(
            &app.database,
            id,
            1,
            "fixture-lease",
            0,
            vec![second, first]
        )
        .await,
        Err(AppError::Conflict(_))
    ));
    assert!(matches!(
        authorization::record_manifest_batch(&app.database, id, 1, "fixture-lease", 2, vec![first])
            .await,
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        authorization::record_manifest_batch(&app.database, id, 1, "fixture-lease", 1, vec![first])
            .await,
        Err(AppError::Conflict(_))
    ));
    let current = reload(&app, id).await;
    assert_eq!(current.meta.row_count, 2);
    let actor = authorization::execution_actor(&app.database, operator_id, "users")
        .await
        .unwrap();
    authorization::verify_user_manifest(&app.database, &current, &actor)
        .await
        .unwrap();
    app.send(
        "PATCH",
        &format!("/api/v1/users/{second}"),
        json!({"version":1,"roleIds":[other]}),
        Some(owner),
    )
    .await;
    assert!(matches!(
        authorization::verify_user_manifest(&app.database, &current, &actor).await,
        Err(AppError::Conflict(_))
    ));
    app.send(
        "PATCH",
        &format!("/api/v1/users/{second}"),
        json!({"version":2,"roleIds":[team]}),
        Some(owner),
    )
    .await;
    authorization::verify_user_manifest(&app.database, &current, &actor)
        .await
        .unwrap();
    app.send(
        "PATCH",
        &format!("/api/v1/roles/{operator_role}"),
        json!({"version":1,"dataScope":"self","dataScopeRoleIds":[]}),
        Some(owner),
    )
    .await;
    let narrowed = authorization::execution_actor(&app.database, operator_id, "users")
        .await
        .unwrap();
    assert!(matches!(
        authorization::verify_user_manifest(&app.database, &current, &narrowed).await,
        Err(AppError::Conflict(_))
    ));
    assert_eq!(
        app.send(
            "POST",
            &format!("/api/v1/data-tools/exports/{id}/cancel"),
            json!({}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(matches!(
        authorization::record_manifest_batch(&app.database, id, 1, "fixture-lease", 1, vec![first])
            .await,
        Err(AppError::Conflict(_))
    ));
    app.cleanup().await;
}

#[tokio::test]
async fn later_expansion_does_not_expand_the_creation_filter_and_missing_manifests_fail_closed() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let owner = auth["accessToken"].as_str().unwrap();
    let role = role(&app, owner, "creation-scope", "self", vec![]).await;
    let (id, token) = user(&app, owner, "creation-scope", &role).await;
    let job = claimed(&app, &token).await;
    app.send(
        "PATCH",
        &format!("/api/v1/roles/{role}"),
        json!({"version":1,"dataScope":"all"}),
        Some(owner),
    )
    .await;
    let expanded = authorization::execution_actor(&app.database, id, "users")
        .await
        .unwrap();
    let filter = authorization::user_filter(&job, &expanded);
    assert_eq!(
        app.database
            .collection::<mongodb::bson::Document>("users")
            .count_documents(doc! {"$and":[{"deletedAt":null},filter]})
            .await
            .unwrap(),
        1
    );
    let mut missing = job.clone();
    missing.meta.row_count = 1;
    assert!(matches!(
        authorization::verify_user_manifest(&app.database, &missing, &expanded).await,
        Err(AppError::NotReady)
    ));
    app.cleanup().await;
}

#[tokio::test]
async fn manifest_verification_crosses_cursor_batches_and_rejects_gaps_and_expired_leases() {
    let app = TestApp::new().await;
    let auth = app.login().await;
    let token = auth["accessToken"].as_str().unwrap();
    let owner = ObjectId::parse_str(auth["user"]["id"].as_str().unwrap()).unwrap();
    let ids = (0..9001).map(|_| ObjectId::new()).collect::<Vec<_>>();
    let now = DateTime::now();
    let users=ids.iter().enumerate().map(|(index,id)|doc!{"_id":*id,"username":format!("manifest-{index:05}"),"usernameKey":format!("manifest-{index:05}"),"displayName":"Manifest fixture","roleIds":[],"status":"active","version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null}).collect::<Vec<_>>();
    app.database
        .collection::<mongodb::bson::Document>("users")
        .insert_many(users)
        .await
        .unwrap();
    let job = claimed(&app, token).await;
    for (batch, rows) in ids.chunks(1000).enumerate() {
        assert!(
            authorization::record_manifest_batch(
                &app.database,
                job.meta.id,
                1,
                "fixture-lease",
                batch as u32,
                rows.to_vec()
            )
            .await
            .unwrap()
        );
    }
    let complete = reload(&app, job.meta.id).await;
    assert_eq!(complete.meta.row_count, 9001);
    let actor = authorization::execution_actor(&app.database, owner, "users")
        .await
        .unwrap();
    authorization::verify_user_manifest(&app.database, &complete, &actor)
        .await
        .unwrap();
    app.database
        .collection::<mongodb::bson::Document>("data_exports")
        .update_one(
            doc! {"_id":job.meta.id},
            doc! {"$set":{"leaseExpiresAt":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(matches!(
        authorization::record_manifest_batch(
            &app.database,
            job.meta.id,
            1,
            "fixture-lease",
            10,
            vec![owner]
        )
        .await,
        Err(AppError::Conflict(_))
    ));
    app.database
        .collection::<mongodb::bson::Document>("data_export_manifest")
        .delete_one(doc! {"jobId":job.meta.id,"generation":1i64,"batch":4})
        .await
        .unwrap();
    assert!(matches!(
        authorization::verify_user_manifest(&app.database, &complete, &actor).await,
        Err(AppError::NotReady)
    ));
    app.cleanup().await;
}
