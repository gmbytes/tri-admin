use super::{sessions::PrincipalKind, usage::RETENTION_MS};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
    },
    modules::applications::models::time,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use serde::{Deserialize, Serialize};
fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CallState {
    Started,
    Finished,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    #[serde(default = "first")]
    pub page: u64,
    #[serde(default = "size")]
    pub page_size: u64,
    pub principal_kind: Option<PrincipalKind>,
    pub user_id: Option<String>,
    pub state: Option<CallState>,
    pub status: Option<u16>,
    pub from: Option<String>,
    pub to: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    #[serde(rename = "_id")]
    id: ObjectId,
    application_id: ObjectId,
    principal_kind: PrincipalKind,
    user_id: Option<ObjectId>,
    session_id: ObjectId,
    request_id: String,
    operation_id: Option<String>,
    method: String,
    route: String,
    ip: Option<String>,
    state: CallState,
    status: Option<i32>,
    error_code: Option<String>,
    duration_ms: Option<i64>,
    created_at: DateTime,
    finished_at: Option<DateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    pub application_id: String,
    pub principal_kind: PrincipalKind,
    pub user_id: Option<String>,
    pub session_id: String,
    pub request_id: String,
    pub operation_id: Option<String>,
    pub method: String,
    pub route: String,
    pub ip: Option<String>,
    pub state: CallState,
    pub status: Option<i32>,
    pub error_code: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: String,
    pub finished_at: Option<String>,
}
impl Stored {
    fn view(self) -> Result<View, AppError> {
        if self
            .status
            .is_some_and(|value| !(100..=599).contains(&value))
            || self.duration_ms.is_some_and(|v| v < 0)
        {
            return Err(AppError::NotReady);
        }
        Ok(View {
            id: self.id.to_hex(),
            application_id: self.application_id.to_hex(),
            principal_kind: self.principal_kind,
            user_id: self.user_id.map(|v| v.to_hex()),
            session_id: self.session_id.to_hex(),
            request_id: self.request_id,
            operation_id: self.operation_id,
            method: self.method,
            route: self.route,
            ip: self.ip,
            state: self.state,
            status: self.status,
            error_code: self.error_code,
            duration_ms: self.duration_ms,
            created_at: time(self.created_at)?,
            finished_at: self.finished_at.map(time).transpose()?,
        })
    }
}
pub async fn list(
    database: &Database,
    application: ObjectId,
    input: Input,
) -> Result<Page<View>, AppError> {
    let page = PageInput {
        page: input.page,
        page_size: input.page_size,
        search: None,
        sort: None,
    };
    let skip = page.validate()?;
    let mut filter = doc! {"applicationId":application};
    let mut range =
        doc! {"$gt":DateTime::from_millis(DateTime::now().timestamp_millis()-RETENTION_MS)};
    let parse = |text: &str| {
        chrono::DateTime::parse_from_rfc3339(text)
            .map(|v| DateTime::from_millis(v.timestamp_millis()))
            .map_err(|_| AppError::Validation("日志时间无效".into()))
    };
    let from = input.from.as_deref().map(parse).transpose()?;
    let to = input.to.as_deref().map(parse).transpose()?;
    if from.zip(to).is_some_and(|(from, to)| from > to)
        || input.status.is_some_and(|v| !(100..=599).contains(&v))
    {
        return Err(AppError::Validation("日志筛选无效".into()));
    }
    if let Some(from) = from {
        range.insert("$gte", from);
    }
    if let Some(to) = to {
        range.insert("$lte", to);
    }
    filter.insert("createdAt", range);
    if let Some(kind) = input.principal_kind {
        filter.insert("principalKind", kind.as_str());
    }
    if let Some(user) = input.user_id {
        filter.insert("userId", crate::common::validation::object_id(&user)?);
    }
    if let Some(status) = input.status {
        filter.insert("status", i32::from(status));
    }
    if let Some(state) = input.state {
        filter.insert(
            "state",
            match state {
                CallState::Started => "started",
                CallState::Finished => "finished",
            },
        );
    }
    let (rows, total) = transaction::run_snapshot(
        database,
        (database.clone(), filter),
        |session, (database, filter)| {
            async move {
                if database
                    .collection::<Document>("applications")
                    .find_one(doc! {"_id":application})
                    .projection(doc! {"_id":1})
                    .session(&mut *session)
                    .await?
                    .is_none()
                {
                    return Err(fail(Failure::NotFound));
                }
                let total = database
                    .collection::<Document>("application_call_logs")
                    .count_documents(filter.clone())
                    .session(&mut *session)
                    .await?;
                let mut cursor = database
                    .collection::<Stored>("application_call_logs")
                    .find(filter.clone())
                    .sort(doc! {"createdAt":-1,"_id":-1})
                    .skip(skip)
                    .limit(input.page_size as i64)
                    .session(&mut *session)
                    .await?;
                Ok((cursor.stream(session).try_collect::<Vec<_>>().await?, total))
            }
            .boxed()
        },
    )
    .await?;
    Ok(Page::new(
        rows.into_iter()
            .map(Stored::view)
            .collect::<Result<Vec<_>, _>>()?,
        &page,
        total,
    ))
}
