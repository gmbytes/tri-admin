use crate::{Error, State, auth, schema::SAFE_INTEGER, transaction};
use axum::{
    Json, Router,
    extract::{
        ConnectInfo, DefaultBodyLimit, FromRequest, Path, Query, State as Extract,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::cors::CorsLayer;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    values: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    version: i64,
    values: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Paging {
    #[serde(default = "first")]
    page: u64,
    #[serde(default = "size")]
    page_size: u64,
    search: Option<String>,
}
fn body<T>(input: Result<Json<T>, JsonRejection>) -> Result<T, Error> {
    input.map(|Json(v)| v).map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Error::PayloadTooLarge
        } else {
            Error::Validation
        }
    })
}
fn id(raw: &str) -> Result<ObjectId, Error> {
    let id = ObjectId::parse_str(raw).map_err(|_| Error::Validation)?;
    if id.to_hex() != raw {
        return Err(Error::Validation);
    }
    Ok(id)
}
fn version(value: i64) -> Result<(), Error> {
    if (1..SAFE_INTEGER).contains(&value) {
        Ok(())
    } else {
        Err(Error::Validation)
    }
}
pub fn router(state: State) -> Router {
    let path = format!("/api/v1/{}", state.schema.resource);
    let item = format!("{path}/{{id}}");
    let origin = HeaderValue::from_str(&state.config.origin).expect("validated origin");
    Router::new()
        .route("/health/ready", get(ready))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/me", get(me))
        .route("/api/v1/auth/logout", post(logout))
        .route(&path, get(list).post(create))
        .route(&item, get(one).patch(update).delete(remove))
        .fallback(|| async { Error::NotFound })
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(origin)
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PATCH,
                    axum::http::Method::DELETE,
                ])
                .allow_headers([
                    axum::http::header::AUTHORIZATION,
                    axum::http::header::CONTENT_TYPE,
                ]),
        )
        .layer(middleware::from_fn(headers))
        .with_state(state)
}
async fn headers(request: Request<axum::body::Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "cache-control",
        HeaderValue::from_static("private, no-store"),
    );
    if let Ok(value) = HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}
async fn ready(Extract(state): Extract<State>) -> Result<Json<Value>, Error> {
    state.ready().await?;
    Ok(Json(json!({"data":{"ready":true}})))
}
async fn login(
    Extract(state): Extract<State>,
    request: Request<axum::body::Body>,
) -> Result<Json<Value>, Error> {
    let peer = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|v| v.0.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    let input = Json::<auth::Login>::from_request(request, &state).await;
    Ok(Json(
        json!({"data":auth::login(&state,body(input)?,&peer).await?}),
    ))
}
async fn me(Extract(state): Extract<State>, headers: HeaderMap) -> Result<Json<Value>, Error> {
    let claims = auth::claims(&state, &headers)?;
    let view = transaction::run(
        &state.database,
        (state.clone(), claims),
        true,
        |session, (state, claims)| {
            async move { Ok(auth::resolve(state, session, claims, false).await?.view()) }.boxed()
        },
    )
    .await?;
    Ok(Json(json!({"data":view})))
}
async fn logout(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    input: Result<Json<Empty>, JsonRejection>,
) -> Result<StatusCode, Error> {
    let claims = auth::claims(&state, &headers)?;
    body(input)?;
    transaction::run(
        &state.database,
        (state.clone(), claims),
        false,
        |session, (state, claims)| {
            async move {
                let actor = auth::resolve(state, session, claims, false).await?;
                state
                    .database
                    .collection::<Document>("_resource_admins")
                    .update_one(doc! {"_id":actor.id}, doc! {"$inc":{"writeFence":1i64}})
                    .session(&mut *session)
                    .await?;
                state
                    .database
                    .collection::<Document>("_resource_sessions")
                    .update_one(doc! {"_id":actor.sid}, doc! {"$set":{"revoked":true}})
                    .session(&mut *session)
                    .await?;
                auth::audit(state, session, actor.id, "auth.logout", None).await?;
                Ok(())
            }
            .boxed()
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
fn filter(actor: &auth::Actor) -> Document {
    let mut filter = doc! {"deletedAt":null};
    if !actor.all {
        filter.insert("ownerId", actor.id);
    }
    filter
}
fn public(state: &State, row: &Document) -> Result<Value, Error> {
    let time = |field| -> Result<String, Error> {
        let value = row
            .get_datetime(field)
            .map_err(|_| Error::Internal)?
            .timestamp_millis();
        Ok(chrono::DateTime::from_timestamp_millis(value)
            .ok_or(Error::Internal)?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    };
    let values = state
        .schema
        .public_values(row.get_document("values").map_err(|_| Error::Internal)?)
        .map_err(|_| Error::Internal)?;
    Ok(
        json!({"id":row.get_object_id("_id").map_err(|_|Error::Internal)?.to_hex(),"ownerId":row.get_object_id("ownerId").map_err(|_|Error::Internal)?.to_hex(),"version":row.get_i64("version").map_err(|_|Error::Internal)?,"createdAt":time("createdAt")?,"updatedAt":time("updatedAt")?,"values":values}),
    )
}
fn prefix(raw: &str) -> String {
    let mut output = String::from("^");
    for c in raw.chars() {
        if ".*+?^$()[]{}|\\".contains(c) {
            output.push('\\')
        }
        output.push(c)
    }
    output
}
async fn list(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    input: Result<Query<Paging>, QueryRejection>,
) -> Result<Json<Value>, Error> {
    let claims = auth::claims(&state, &headers)?;
    let Query(input) = input.map_err(|_| Error::Validation)?;
    if !(1..=100_000).contains(&input.page)
        || !(1..=100).contains(&input.page_size)
        || input
            .search
            .as_ref()
            .is_some_and(|v| v.chars().count() > 100)
    {
        return Err(Error::Validation);
    }
    let search = input.search.as_deref().unwrap_or("").trim();
    let searches = state
        .schema
        .fields
        .iter()
        .filter(|v| v.searchable)
        .map(|field| {
            let mut condition = Document::new();
            condition.insert(
                format!("values.{}", field.name),
                doc! {"$regex":prefix(search)},
            );
            condition
        })
        .collect::<Vec<_>>();
    if !search.is_empty() && searches.is_empty() {
        return Err(Error::Validation);
    }
    let page = input.page;
    let page_size = input.page_size;
    let (rows, total) = transaction::run(
        &state.database,
        (state.clone(), claims, search.to_owned(), searches),
        true,
        |session, (state, claims, search, searches)| {
            async move {
                let actor = auth::resolve(state, session, claims, false).await?;
                let mut query = filter(&actor);
                if !search.is_empty() {
                    query.insert("$or", searches.clone());
                }
                let collection = state
                    .database
                    .collection::<Document>(&state.schema.collection());
                let total = collection
                    .count_documents(query.clone())
                    .session(&mut *session)
                    .await?;
                let mut cursor = collection
                    .find(query)
                    .sort(doc! {"createdAt":-1,"_id":-1})
                    .skip((page - 1) * page_size)
                    .limit(page_size as i64)
                    .session(&mut *session)
                    .await?;
                let rows = cursor.stream(session).try_collect::<Vec<_>>().await?;
                Ok((rows, total))
            }
            .boxed()
        },
    )
    .await?;
    let rows = rows
        .iter()
        .map(|row| public(&state, row))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(
        json!({"data":rows,"meta":{"page":page,"pageSize":page_size,"total":total}}),
    ))
}
async fn one(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Result<Json<Value>, Error> {
    let claims = auth::claims(&state, &headers)?;
    let id = id(&raw)?;
    let row = transaction::run(
        &state.database,
        (state.clone(), claims),
        true,
        |session, (state, claims)| {
            async move {
                let actor = auth::resolve(state, session, claims, false).await?;
                let mut query = filter(&actor);
                query.insert("_id", id);
                state
                    .database
                    .collection::<Document>(&state.schema.collection())
                    .find_one(query)
                    .session(session)
                    .await?
                    .ok_or_else(|| Error::NotFound.mongo())
            }
            .boxed()
        },
    )
    .await?;
    Ok(Json(json!({"data":public(&state,&row)?})))
}
async fn create(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    input: Result<Json<Create>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), Error> {
    let claims = auth::claims(&state, &headers)?;
    let values = state.schema.values(&body(input)?.values)?;
    let id = ObjectId::new();
    let now = DateTime::now();
    let row=transaction::run(&state.database,(state.clone(),claims,values),false,|session,(state,claims,values)|async move{let actor=auth::resolve(state,session,claims,true).await?;let row=doc!{"_id":id,"ownerId":actor.id,"version":1i64,"createdAt":now,"updatedAt":now,"deletedAt":null,"values":values.clone()};state.database.collection::<Document>(&state.schema.collection()).insert_one(&row).session(&mut *session).await?;auth::audit(state,session,actor.id,"create",Some(id)).await?;Ok(row)}.boxed()).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"data":public(&state,&row)?})),
    ))
}
async fn update(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    input: Result<Json<Update>, JsonRejection>,
) -> Result<Json<Value>, Error> {
    let claims = auth::claims(&state, &headers)?;
    let id = id(&raw)?;
    let input = body(input)?;
    version(input.version)?;
    let values = state.schema.values(&input.values)?;
    let expected = input.version;
    let row=transaction::run(&state.database,(state.clone(),claims,values),false,|session,(state,claims,values)|async move{let actor=auth::resolve(state,session,claims,true).await?;let mut query=filter(&actor);query.insert("_id",id);let collection=state.database.collection::<Document>(&state.schema.collection());let mut row=collection.find_one(query.clone()).session(&mut *session).await?.ok_or_else(||Error::NotFound.mongo())?;if row.get_i64("version").ok()!=Some(expected){return Err(Error::Conflict.mongo())}query.insert("version",expected);let now=DateTime::now();let changed=collection.update_one(query,doc!{"$set":{"values":values.clone(),"updatedAt":now},"$inc":{"version":1i64}}).session(&mut *session).await?;if changed.matched_count!=1{return Err(Error::Conflict.mongo())}auth::audit(state,session,actor.id,"update",Some(id)).await?;row.insert("values",values.clone());row.insert("updatedAt",now);row.insert("version",expected+1);Ok(row)}.boxed()).await?;
    Ok(Json(json!({"data":public(&state,&row)?})))
}
async fn remove(
    Extract(state): Extract<State>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    input: Result<Query<Delete>, QueryRejection>,
) -> Result<StatusCode, Error> {
    let claims = auth::claims(&state, &headers)?;
    let id = id(&raw)?;
    let Query(input) = input.map_err(|_| Error::Validation)?;
    version(input.version)?;
    let expected = input.version;
    transaction::run(&state.database,(state.clone(),claims),false,|session,(state,claims)|async move{let actor=auth::resolve(state,session,claims,true).await?;let mut query=filter(&actor);query.insert("_id",id);let collection=state.database.collection::<Document>(&state.schema.collection());let row=collection.find_one(query.clone()).session(&mut *session).await?.ok_or_else(||Error::NotFound.mongo())?;if row.get_i64("version").ok()!=Some(expected){return Err(Error::Conflict.mongo())}query.insert("version",expected);collection.update_one(query,doc!{"$set":{"deletedAt":DateTime::now(),"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;auth::audit(state,session,actor.id,"delete",Some(id)).await?;Ok(())}.boxed()).await?;
    Ok(StatusCode::NO_CONTENT)
}
