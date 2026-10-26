use crate::{
    common::error::{AppError, ResponseError},
    modules::api_catalog,
    state::AppState,
};
use axum::{
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::HeaderValue,
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::FutureExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};
use std::{
    net::SocketAddr,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Details {
    actor: Option<ObjectId>,
    username: Option<String>,
    login_recorded: bool,
    event_ids: Vec<ObjectId>,
    call_ticket: Option<crate::modules::open_access::usage::Ticket>,
}
#[derive(Clone)]
struct Context {
    id: String,
    method: &'static str,
    route: &'static str,
    action: Option<&'static str>,
    ip: Option<String>,
    details: Arc<Mutex<Details>>,
}
tokio::task_local! {static CURRENT: Context;}

pub fn request_id() -> Option<String> {
    CURRENT.try_with(|context| context.id.clone()).ok()
}
pub fn call_descriptor() -> Option<crate::modules::open_access::usage::Descriptor> {
    CURRENT
        .try_with(|context| crate::modules::open_access::usage::Descriptor {
            request_id: context.id.clone(),
            operation_id: context.action,
            method: context.method,
            route: context.route,
        })
        .ok()
}
pub fn call_ticket(ticket: crate::modules::open_access::usage::Ticket) {
    let _ = CURRENT.try_with(|context| {
        if let Ok(mut details) = context.details.lock() {
            details.call_ticket = Some(ticket);
        }
    });
}

#[derive(Clone)]
pub(crate) struct StreamErrorReporter {
    database: Database,
    context: Context,
    start: Instant,
    reported: Arc<std::sync::atomic::AtomicBool>,
}
pub(crate) fn stream_reporter(database: &Database) -> Option<StreamErrorReporter> {
    CURRENT
        .try_with(|context| StreamErrorReporter {
            database: database.clone(),
            context: context.clone(),
            start: Instant::now(),
            reported: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
        .ok()
}
impl StreamErrorReporter {
    pub(crate) fn record(&self, code: &'static str) {
        if self
            .reported
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let context = &self.context;
        let actor = context
            .details
            .lock()
            .ok()
            .and_then(|details| details.actor);
        let document = doc! {
            "kind":"error","action":context.action.unwrap_or("request.unknown"),"outcome":"failure",
            "requestId":&context.id,"method":context.method,"route":context.route,"ip":context.ip.clone(),"actorId":actor,
            "status":200,"errorCode":code,"message":"文件内容传输失败，响应头已发送","phase":"responseBody",
            "durationMs":i64::try_from(self.start.elapsed().as_millis()).unwrap_or(i64::MAX),"createdAt":DateTime::now()
        };
        let database = self.database.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_millis(500), async {
                if database
                    .collection::<Document>("_schema_migrations")
                    .find_one(doc! {"_id":"logs","version":crate::storage::logs_schema::VERSION})
                    .await?
                    .is_some()
                {
                    database
                        .collection::<Document>("request_errors")
                        .insert_one(document)
                        .await?;
                }
                Ok::<_, mongodb::error::Error>(())
            })
            .await;
        });
    }
}

pub fn actor(id: ObjectId) {
    let _ = CURRENT.try_with(|context| {
        if let Ok(mut details) = context.details.lock() {
            details.actor = Some(id);
        }
    });
}
pub fn login_username(username: &str) {
    let _ = CURRENT.try_with(|context| {
        if let Ok(mut details) = context.details.lock() {
            details.username = Some(username.to_owned());
        }
    });
}
pub fn login_recorded() {
    let _ = CURRENT.try_with(|context| {
        if let Ok(mut details) = context.details.lock() {
            details.login_recorded = true;
        }
    });
}

/// Adds only trusted request metadata; no body, query, arbitrary header or raw URI.
pub fn event(mut document: Document) -> Document {
    let kind = if document.get_str("action").ok() == Some("auth.login") {
        "login"
    } else {
        "operation"
    };
    document.insert("kind", kind);
    let id = document
        .get_object_id("_id")
        .unwrap_or_else(|_| ObjectId::new());
    document.insert("_id", id);
    let _ = CURRENT.try_with(|context| {
        document.insert("requestId", &context.id);
        document.insert("method", context.method);
        document.insert("route", context.route);
        if let Some(action) = context.action {
            document.insert("operationId", action);
        }
        if let Some(ip) = &context.ip {
            document.insert("ip", ip);
        }
        if let Ok(mut details) = context.details.lock()
            && details.event_ids.len() < 256
        {
            details.event_ids.push(id);
        }
    });
    document
}

fn route(request: &Request) -> (&'static str, Option<&'static str>) {
    static ENDPOINTS: OnceLock<Vec<crate::common::api::Endpoint>> = OnceLock::new();
    let endpoints = ENDPOINTS.get_or_init(api_catalog::endpoints);
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("");
    let Some(known) = endpoints.iter().find(|endpoint| endpoint.path == path) else {
        return ("<unmatched>", None);
    };
    let method = if request.method() == axum::http::Method::HEAD {
        "GET"
    } else {
        request.method().as_str()
    };
    (
        known.path,
        endpoints
            .iter()
            .find(|endpoint| endpoint.path == path && endpoint.method == method)
            .map(|endpoint| endpoint.operation_id),
    )
}

async fn persist(
    database: &Database,
    context: &Context,
    status: u16,
    code: &str,
    duration: i64,
) -> mongodb::error::Result<()> {
    if database
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"logs","version":crate::storage::logs_schema::VERSION})
        .await?
        .is_none()
    {
        return Ok(());
    }
    let (ids, actor, username, recorded) = context
        .details
        .lock()
        .map(|details| {
            (
                details.event_ids.clone(),
                details.actor,
                details.username.clone(),
                details.login_recorded,
            )
        })
        .unwrap_or_default();
    if !ids.is_empty() {
        database
            .collection::<Document>("audit_events")
            .update_many(
                doc! {"_id":{"$in":ids}},
                doc! {"$set":{"status":i32::from(status),"durationMs":duration}},
            )
            .await?;
    }
    if context.action == Some("auth.login") && status >= 400 && !recorded {
        let mut document = event(
            doc! {"category":"authentication","action":"auth.login","outcome":"blocked","status":i32::from(status),"errorCode":code,"createdAt":DateTime::now(),"durationMs":duration},
        );
        if let Some(username) = username {
            document.insert("username", username);
        }
        database
            .collection::<Document>("audit_events")
            .insert_one(document)
            .await?;
    }
    if status >= 400 {
        let document = doc! {"kind":"error","action":context.action.unwrap_or("request.unknown"),"outcome":"failure","requestId":&context.id,"method":context.method,"route":context.route,"ip":context.ip.clone(),"actorId":actor,"status":i32::from(status),"errorCode":code,"message":if status>=500{"服务处理失败"}else{"请求未成功"},"durationMs":duration,"createdAt":DateTime::now()};
        database
            .collection::<Document>("request_errors")
            .insert_one(document)
            .await?;
    }
    Ok(())
}

pub async fn observe(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() == 36)
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .unwrap_or_else(uuid::Uuid::new_v4)
        .to_string();
    let (route, action) = route(&request);
    let method = match request.method().as_str() {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "OPTIONS" => "OPTIONS",
        _ => "OTHER",
    };
    let context = Context {
        id: id.clone(),
        method,
        route,
        action,
        ip: request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip().to_string()),
        details: Arc::new(Mutex::new(Details::default())),
    };
    request.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&id).expect("UUID is a valid header"),
    );
    CURRENT.scope(context.clone(),async move{
  let start=Instant::now();let mut response=crate::common::error::normalize_framework(match AssertUnwindSafe(next.run(request)).catch_unwind().await{Ok(response)=>response,Err(_)=>AppError::Internal.into_response()});
  let duration=i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);let status=response.status().as_u16();
  let code=response.extensions().get::<ResponseError>().map(|error|error.code.to_owned()).unwrap_or_else(||format!("HTTP_{status}"));
  let ticket=context.details.lock().ok().and_then(|details|details.call_ticket.clone());
  if let Some(ticket)=ticket {
    response.headers_mut().insert(axum::http::header::CACHE_CONTROL,HeaderValue::from_static("private, no-store"));
    if let Some(database)=&state.database && !matches!(tokio::time::timeout(Duration::from_millis(500),crate::modules::open_access::usage::finish(database,&ticket,status,&code,duration)).await,Ok(Ok(()))){tracing::warn!(request_id=%context.id,"API call completion remains unconfirmed");}
  }
  let has_events=context.details.lock().is_ok_and(|details|!details.event_ids.is_empty());
  if (status>=400||has_events)&&let Some(database)=&state.database
   && !matches!(tokio::time::timeout(Duration::from_millis(500),persist(database,&context,status,&code,duration)).await,Ok(Ok(()))) {tracing::warn!(request_id=%context.id,"request log storage unavailable");}
  tracing::debug!(request_id=%context.id,method=context.method,route=context.route,status,duration_ms=duration,"request completed");
  response.headers_mut().insert("x-request-id",HeaderValue::from_str(&id).expect("UUID is a valid header"));response
 }).await
}
