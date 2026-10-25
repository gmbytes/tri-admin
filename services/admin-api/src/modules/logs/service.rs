use super::{
    dto::{CleanupInput, Kind, LogInput},
    models::{CleanupResult, LogView, Record, time},
};
use crate::{
    common::{
        canonical,
        error::AppError,
        pagination::Page,
        transaction::{self, Failure, fail},
        validation,
    },
    modules::rbac::shared::{self, Actor},
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use std::collections::{HashMap, HashSet};

fn timestamp(raw: &str) -> Result<DateTime, AppError> {
    if raw.len() > 64 {
        return Err(AppError::Validation("时间格式无效".into()));
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|time| DateTime::from_millis(time.timestamp_millis()))
        .map_err(|_| AppError::Validation("时间需为RFC3339格式".into()))
}
fn request_id(raw: &str) -> Result<String, AppError> {
    uuid::Uuid::parse_str(raw)
        .map(|id| id.to_string())
        .map_err(|_| AppError::Validation("requestId需为UUID".into()))
}
fn filter(input: &LogInput) -> Result<Document, AppError> {
    let mut filter = doc! {"kind":input.kind.as_str()};
    if let Some(action) = &input.action {
        if action.is_empty()
            || action.len() > 100
            || !action
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err(AppError::Validation("动作标识无效".into()));
        }
        filter.insert("action", action);
    }
    if let Some(username) = &input.username {
        if input.kind != Kind::Login || username.trim().is_empty() || username.chars().count() > 64
        {
            return Err(AppError::Validation(
                "账号筛选只用于登录日志且最多64字符".into(),
            ));
        }
        filter.insert("username", username.trim().to_lowercase());
    }
    if let Some(actor) = &input.actor_id {
        filter.insert("actorId", validation::object_id(actor)?);
    }
    if let Some(id) = &input.request_id {
        filter.insert("requestId", request_id(id)?);
    }
    if let Some(outcome) = &input.outcome {
        if !["success", "failure", "blocked", "revoked"].contains(&outcome.as_str()) {
            return Err(AppError::Validation("日志结果无效".into()));
        }
        filter.insert("outcome", outcome);
    }
    if let Some(status) = input.status {
        if !(100..=599).contains(&status) {
            return Err(AppError::Validation("HTTP状态无效".into()));
        }
        filter.insert("status", i32::from(status));
    }
    let from = input.from.as_deref().map(timestamp).transpose()?;
    let to = input.to.as_deref().map(timestamp).transpose()?;
    if from.zip(to).is_some_and(|(from, to)| from >= to) {
        return Err(AppError::Validation("结束时间必须晚于开始时间".into()));
    }
    let mut range = Document::new();
    if let Some(from) = from {
        range.insert("$gte", from);
    }
    if let Some(to) = to {
        range.insert("$lt", to);
    }
    if !range.is_empty() {
        filter.insert("createdAt", range);
    }
    Ok(filter)
}
pub async fn list(database: &Database, input: LogInput) -> Result<Page<LogView>, AppError> {
    let page = input.page();
    let skip = page.validate()?;
    let sort = page.sort_document(&["createdAt"])?;
    let filter = filter(&input)?;
    let total = database
        .collection::<Document>(input.kind.collection())
        .count_documents(filter.clone())
        .await?;
    let records = database
        .collection::<Record>(input.kind.collection())
        .find(filter)
        .sort(sort)
        .skip(skip)
        .limit(input.page_size as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let ids = records
        .iter()
        .filter_map(|row| row.actor_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut names = HashMap::new();
    if !ids.is_empty() {
        let users = database
            .collection::<Document>("users")
            .find(doc! {"_id":{"$in":ids}})
            .projection(doc! {"displayName":1})
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        for row in users {
            if let (Ok(id), Ok(name)) = (row.get_object_id("_id"), row.get_str("displayName")) {
                names.insert(id, name.to_owned());
            }
        }
    }
    let data = records
        .into_iter()
        .map(|row| {
            let name = row.actor_id.and_then(|id| names.get(&id).cloned());
            row.view(name)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(data, &page, total))
}
pub async fn cleanup(
    database: &Database,
    actor: Actor,
    input: CleanupInput,
) -> Result<CleanupResult, AppError> {
    let request_id = request_id(&input.request_id)?;
    let before = timestamp(&input.before)?;
    if !(1..=1000).contains(&input.limit)
        || before.timestamp_millis() > DateTime::now().timestamp_millis() - 86400000
    {
        return Err(AppError::Validation(
            "每批1–1000条，只能清理至少24小时前的记录".into(),
        ));
    }
    let hash=canonical::checksum(canonical::json(&serde_json::json!({"kind":input.kind,"before":before.timestamp_millis(),"limit":input.limit}))?.as_bytes());
    transaction::run_snapshot(database,(database.clone(),actor),|session,(database,actor)|{
 let request_id=request_id.clone();let hash=hash.clone();async move{
  shared::lock(database,"logs",session).await?;
  if let Some(previous)=database.collection::<Document>("log_cleanups").find_one(doc!{"requestId":&request_id}).session(&mut *session).await?{
   if previous.get_object_id("actorId").ok()!=Some(actor.id){return Err(fail(Failure::NotFound));}
   if previous.get_str("inputHash").ok()!=Some(hash.as_str()){return Err(fail(Failure::Conflict("同一清理请求不能使用不同条件")));}
   return serde_json::from_str(previous.get_str("resultJson").map_err(|_|fail(Failure::NotReady))?).map_err(|_|fail(Failure::NotReady));
  }
  let filter=doc!{"kind":input.kind.as_str(),"createdAt":{"$lt":before}};
  let mut cursor=database.collection::<Document>(input.kind.collection()).find(filter.clone()).sort(doc!{"createdAt":1,"_id":1}).projection(doc!{"_id":1}).limit(i64::from(input.limit)).session(&mut *session).await?;
  let rows=cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
  let ids=rows.iter().map(|row|row.get_object_id("_id").map_err(|_|fail(Failure::NotReady))).collect::<Result<Vec<_>,_>>()?;
  let deleted=if ids.is_empty(){0}else{database.collection::<Document>(input.kind.collection()).delete_many(doc!{"_id":{"$in":ids}}).session(&mut *session).await?.deleted_count};
  let more=database.collection::<Document>(input.kind.collection()).find_one(filter).session(&mut *session).await?.is_some();let now=DateTime::now();
  let result=CleanupResult{request_id:request_id.clone(),kind:input.kind,before:time(before).map_err(|_|fail(Failure::NotReady))?,deleted_count:deleted,has_more:more,completed_at:time(now).map_err(|_|fail(Failure::NotReady))?};let id=ObjectId::new();let json=canonical::json(&result).map_err(|_|fail(Failure::NotReady))?;
  database.collection::<Document>("log_cleanups").insert_one(doc!{"_id":id,"requestId":request_id,"actorId":actor.id,"inputHash":hash,"resultJson":json,"createdAt":now}).session(&mut *session).await?;
  shared::audit(database,actor,"logs.cleanup",id,session).await?;Ok(result)
 }.boxed()}).await
}
