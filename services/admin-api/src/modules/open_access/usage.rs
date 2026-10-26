use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        transaction,
    },
    modules::open_access::credential,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
    options::ReturnDocument,
};
use serde::{Deserialize, Serialize};
pub const RETENTION_MS: i64 = 30 * 86400000;
#[derive(Clone)]
pub struct Descriptor {
    pub request_id: String,
    pub operation_id: Option<&'static str>,
    pub method: &'static str,
    pub route: &'static str,
}
#[derive(Clone)]
pub enum Ticket {
    AdminToken { id: ObjectId },
    Open { id: ObjectId },
}
pub struct CallStart<'a> {
    pub owner: ObjectId,
    pub token: ObjectId,
    pub peer: Option<std::net::IpAddr>,
    pub descriptor: &'a Descriptor,
    pub now: DateTime,
}
pub async fn begin(
    database: &Database,
    session: &mut ClientSession,
    id: ObjectId,
    call: CallStart<'_>,
) -> mongodb::error::Result<()> {
    let CallStart {
        owner,
        token,
        peer,
        descriptor,
        now,
    } = call;
    database.collection::<Document>("api_call_logs").insert_one(doc!{"_id":id,"ownerId":owner,"tokenId":token,"requestId":&descriptor.request_id,"operationId":descriptor.operation_id,"method":descriptor.method,"route":descriptor.route,"ip":peer.map(|ip|ip.to_string()),"state":"started","status":null,"errorCode":null,"durationMs":null,"createdAt":now,"finishedAt":null,"expireAt":DateTime::from_millis(now.timestamp_millis()+RETENTION_MS)}).session(session).await?;
    Ok(())
}
pub async fn finish(
    database: &Database,
    ticket: &Ticket,
    status: u16,
    code: &str,
    duration: i64,
) -> Result<(), AppError> {
    let (collection, id) = match ticket {
        Ticket::AdminToken { id } => ("api_call_logs", *id),
        Ticket::Open { id } => ("application_call_logs", *id),
    };
    database.collection::<Document>(collection).update_one(doc!{"_id":id,"state":"started"},doc!{"$set":{"state":"finished","status":i32::from(status),"errorCode":if status>=400{Some(code)}else{None},"durationMs":duration,"finishedAt":DateTime::now()}}).await?;
    Ok(())
}
fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallsInput {
    #[serde(default = "first")]
    pub page: u64,
    #[serde(default = "size")]
    pub page_size: u64,
    pub token_id: Option<String>,
    pub status: Option<u16>,
    pub from: Option<String>,
    pub to: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallView {
    pub id: String,
    pub token_id: String,
    pub request_id: String,
    pub operation_id: Option<String>,
    pub method: String,
    pub route: String,
    pub ip: Option<String>,
    pub state: String,
    pub status: Option<i32>,
    pub error_code: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
    pub finished_at: Option<String>,
}
fn time(date: DateTime) -> Result<String, AppError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(date.timestamp_millis())
        .map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or(AppError::NotReady)
}
fn view(row: Document) -> Result<CallView, AppError> {
    let text = |key| {
        row.get_str(key)
            .map(String::from)
            .map_err(|_| AppError::NotReady)
    };
    let optional_text = |key| match row.get(key) {
        Some(mongodb::bson::Bson::Null) => Ok(None),
        Some(mongodb::bson::Bson::String(v)) => Ok(Some(v.clone())),
        _ => Err(AppError::NotReady),
    };
    Ok(CallView {
        id: row
            .get_object_id("_id")
            .map_err(|_| AppError::NotReady)?
            .to_hex(),
        token_id: row
            .get_object_id("tokenId")
            .map_err(|_| AppError::NotReady)?
            .to_hex(),
        request_id: text("requestId")?,
        operation_id: optional_text("operationId")?,
        method: text("method")?,
        route: text("route")?,
        ip: optional_text("ip")?,
        state: text("state")?,
        status: row.get_i32("status").ok(),
        error_code: optional_text("errorCode")?,
        duration_ms: row.get_i64("durationMs").ok(),
        created_at: time(
            *row.get_datetime("createdAt")
                .map_err(|_| AppError::NotReady)?,
        )?,
        finished_at: row
            .get_datetime("finishedAt")
            .ok()
            .copied()
            .map(time)
            .transpose()?,
    })
}
pub async fn list(
    database: &Database,
    owner: ObjectId,
    input: CallsInput,
) -> Result<Page<CallView>, AppError> {
    let page = PageInput {
        page: input.page,
        page_size: input.page_size,
        search: None,
        sort: None,
    };
    let skip = page.validate()?;
    let mut filter = doc! {"ownerId":owner};
    let mut range =
        doc! {"$gt":DateTime::from_millis(DateTime::now().timestamp_millis()-RETENTION_MS)};
    let parse = |v: &str| {
        chrono::DateTime::parse_from_rfc3339(v)
            .map(|v| DateTime::from_millis(v.timestamp_millis()))
            .map_err(|_| AppError::Validation("调用日志时间无效".into()))
    };
    let from = input.from.as_deref().map(parse).transpose()?;
    let to = input.to.as_deref().map(parse).transpose()?;
    if from.zip(to).is_some_and(|(from, to)| from > to)
        || input.status.is_some_and(|s| !(100..=599).contains(&s))
    {
        return Err(AppError::Validation("调用日志筛选无效".into()));
    }
    if let Some(from) = from {
        range.insert("$gte", from);
    }
    if let Some(to) = to {
        range.insert("$lte", to);
    }
    filter.insert("createdAt", range);
    if let Some(id) = input.token_id {
        filter.insert("tokenId", crate::common::validation::object_id(&id)?);
    }
    if let Some(status) = input.status {
        filter.insert("status", i32::from(status));
    }
    let (rows, total) = transaction::run_snapshot(
        database,
        (database.clone(), filter),
        |session, (database, filter)| {
            async move {
                let total = database
                    .collection::<Document>("api_call_logs")
                    .count_documents(filter.clone())
                    .session(&mut *session)
                    .await?;
                let mut cursor = database
                    .collection::<Document>("api_call_logs")
                    .find(filter.clone())
                    .sort(doc! {"createdAt":-1,"_id":-1})
                    .skip(skip)
                    .limit(input.page_size as i64)
                    .session(&mut *session)
                    .await?;
                let rows = cursor.stream(session).try_collect::<Vec<_>>().await?;
                Ok((rows, total))
            }
            .boxed()
        },
    )
    .await?;
    Ok(Page::new(
        rows.into_iter().map(view).collect::<Result<Vec<_>, _>>()?,
        &page,
        total,
    ))
}
pub async fn check_auth_ip(
    database: &Database,
    peer: Option<std::net::IpAddr>,
) -> Result<(), AppError> {
    let id = credential::hash(&format!(
        "pat-auth:{}",
        peer.map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".into())
    ));
    if let Some(row) = database
        .collection::<Document>("open_auth_attempts")
        .find_one(doc! {"_id":id,"resetAt":{"$gt":DateTime::now()},"count":{"$gte":1000i64}})
        .await?
    {
        return Err(AppError::RateLimitedFor(
            ((row
                .get_datetime("resetAt")
                .map_err(|_| AppError::NotReady)?
                .timestamp_millis()
                - DateTime::now().timestamp_millis())
            .max(1) as u64)
                .div_ceil(1000),
        ));
    }
    Ok(())
}
pub async fn failed_auth(
    database: &Database,
    peer: Option<std::net::IpAddr>,
) -> Result<AppError, AppError> {
    let id = credential::hash(&format!(
        "pat-auth:{}",
        peer.map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".into())
    ));
    let now = DateTime::now();
    let reset = DateTime::from_millis(now.timestamp_millis() + 60000);
    let row=database.collection::<Document>("open_auth_attempts").find_one_and_update(doc!{"_id":id},vec![doc!{"$set":{"count":{"$cond":[{"$gt":["$resetAt",now]},{"$add":["$count",1i64]},1i64]},"resetAt":{"$cond":[{"$gt":["$resetAt",now]},"$resetAt",reset]},"expireAt":DateTime::from_millis(now.timestamp_millis()+86460000)}}]).upsert(true).return_document(ReturnDocument::After).await?.ok_or(AppError::NotReady)?;
    Ok(
        if row.get_i64("count").map_err(|_| AppError::NotReady)? > 1000 {
            AppError::RateLimitedFor(60)
        } else {
            AppError::Unauthorized
        },
    )
}
#[derive(Default, Serialize)]
pub struct CleanupReport {
    pub calls: u64,
    pub buckets: u64,
    pub attempts: u64,
}
pub async fn maintenance_once(database: &Database) -> Result<CleanupReport, AppError> {
    let mut report = CleanupReport::default();
    let now = DateTime::now();
    for collection in [
        "api_call_logs",
        "application_call_logs",
        "api_usage_buckets",
        "application_usage_buckets",
        "open_auth_attempts",
    ] {
        let logs = collection.ends_with("call_logs");
        let eligible = if logs {
            doc! {"createdAt":{"$lte":DateTime::from_millis(now.timestamp_millis()-RETENTION_MS)}}
        } else {
            doc! {"expireAt":{"$lte":now}}
        };
        let ids = database
            .collection::<Document>(collection)
            .find(eligible.clone())
            .projection(doc! {"_id":1})
            .limit(100)
            .await?
            .try_collect::<Vec<_>>()
            .await?
            .into_iter()
            .map(|mut row| row.remove("_id").ok_or(AppError::NotReady))
            .collect::<Result<Vec<_>, _>>()?;
        if !ids.is_empty() {
            let count = database
                .collection::<Document>(collection)
                .delete_many(doc! {"$and":[eligible,{"_id":{"$in":ids}}]})
                .await?
                .deleted_count;
            if logs {
                report.calls += count;
            } else if collection.ends_with("buckets") {
                report.buckets += count;
            } else {
                report.attempts += count;
            }
        }
    }
    Ok(report)
}
pub fn start_service(state: crate::state::AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while !state.open_access_shutdown.is_cancelled() {
            if let Some(database) = state.database.as_ref()
                && crate::storage::schema::is_initialized(database)
                    .await
                    .unwrap_or(false)
                && maintenance_once(database).await.is_err()
            {
                tracing::warn!("API usage maintenance deferred");
            }
            tokio::select! {_=state.open_access_shutdown.cancelled()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(60))=>{}}
        }
    })
}
pub fn stop_service(state: &crate::state::AppState) {
    state.open_access_shutdown.cancel();
}
