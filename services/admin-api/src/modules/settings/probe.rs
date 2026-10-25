use super::{
    adapters,
    check_model::{CheckRecord, CheckView, fingerprint},
    models::Profile,
    service, validate,
};
use crate::{
    common::{
        error::AppError,
        observability,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
    },
    modules::rbac::shared::{self, Actor},
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use std::time::{Duration, Instant};
pub async fn execute(
    state: &AppState,
    actor: Actor,
    id: ObjectId,
    version: i64,
) -> Result<CheckView, AppError> {
    validate::version(version)?;
    let _permit = state
        .connection_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let database = state.require_database()?;
    let row = database
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?;
    if row.version != version {
        return Err(AppError::Conflict("配置已变化，请刷新后检查".into()));
    }
    let public = row.configuration()?;
    let secret = service::decrypt(&state.config, &row)?;
    validate::credentials(&public, secret.as_ref(), true)?;
    let snapshot = fingerprint(&row, &state.config)?;
    let created_at = DateTime::now();
    let start = Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(8),
        adapters::run(&state.config, &public, secret.as_ref()),
    )
    .await
    .unwrap_or_else(|_| {
        adapters::Outcome::failed(
            match row.kind {
                super::dto::Kind::MongoDb => "mongodbPing",
                super::dto::Kind::LocalStorage => "localWriteReadDelete",
                super::dto::Kind::S3 => "s3BucketHead",
                super::dto::Kind::Smtp => "smtpSession",
                super::dto::Kind::Http => "httpHead",
            },
            "CHECK_TIMEOUT",
        )
    });
    drop(secret);
    let record = CheckRecord {
        id: ObjectId::new(),
        profile_id: id,
        profile_version: version,
        actor_id: actor.id,
        kind: row.kind,
        fingerprint: snapshot,
        success: outcome.success,
        scope: outcome.scope.into(),
        error_code: outcome.error.map(str::to_owned),
        upstream_status: outcome.status.map(i32::from),
        duration_ms: i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX),
        created_at,
        completed_at: DateTime::now(),
        request_id: observability::request_id(),
    };
    let current=transaction::run(database,(database.clone(),actor),|session,(database,actor)|{let record=record.clone();let config=state.config.clone();async move{
  shared::lock(database,"settings",session).await?;let current=database.collection::<Profile>("integration_profiles").find_one(doc!{"_id":id,"deletedAt":null}).session(&mut *session).await?;
  let matching=current.as_ref().is_some_and(|row|row.version==version&&fingerprint(row,&config).ok().as_deref()==Some(record.fingerprint.as_str()));
  let bson=mongodb::bson::serialize_to_document(&record).map_err(|_|fail(Failure::NotReady))?;
  database.collection::<Document>("configuration_checks").insert_one(bson.clone()).session(&mut *session).await?;
  if matching&&current.as_ref().is_some_and(|row|row.last_check.as_ref().is_none_or(|previous|(previous.completed_at.timestamp_millis(),previous.id.to_hex())<=(record.completed_at.timestamp_millis(),record.id.to_hex()))){database.collection::<Document>("integration_profiles").update_one(doc!{"_id":id,"version":version},doc!{"$set":{"lastCheck":bson}}).session(&mut *session).await?;}
  database.collection::<Document>("audit_events").insert_one(observability::event(doc!{"category":"administration","action":"settings.profiles.probe","actorId":actor.id,"targetId":id,"checkId":record.id,"profileVersion":version,"outcome":if record.success{"success"}else{"failure"},"message":if matching{"连接检查已完成"}else{"连接检查已完成，配置已变化"},"createdAt":DateTime::now()})).session(session).await?;
  Ok(matching)
 }.boxed()}).await?;
    record.view(current)
}
pub async fn history(
    state: &AppState,
    id: ObjectId,
    input: PageInput,
) -> Result<Page<CheckView>, AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt"])?;
    if input.search.is_some() {
        return Err(AppError::Validation("检查记录不支持模糊搜索".into()));
    }
    let database = state.require_database()?;
    let profile = database
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .await?
        .ok_or(AppError::NotFound)?;
    let current = fingerprint(&profile, &state.config)?;
    let filter = doc! {"profileId":id};
    let total = database
        .collection::<Document>("configuration_checks")
        .count_documents(filter.clone())
        .await?;
    let rows = database
        .collection::<CheckRecord>("configuration_checks")
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let data = rows
        .into_iter()
        .map(|row| row.view(row.profile_version == profile.version && row.fingerprint == current))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(data, &input, total))
}
