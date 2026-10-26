use super::{
    auth,
    context::{self, OpenContext},
    scopes::Scope,
    sessions::PrincipalKind,
};
use crate::{
    common::{
        error::AppError,
        request,
        response::ApiResponse,
        transaction::{self, Failure, fail},
    },
    modules::applications::{
        models::{UserMeta, UserView},
        users,
    },
    state::AppState,
};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode, header::CACHE_CONTROL},
    middleware::Next,
    response::Response,
};
use futures_util::FutureExt;
use mongodb::bson::{DateTime, Document, doc};
use serde::Deserialize;
crate::api_routes! {
    prefix "";
    post "/open/v1/auth/application"=>application,"open.auth.application","openAuth","应用服务认证",Public,&[];
    post "/open/v1/auth/login"=>login,"open.auth.login","openAuth","应用用户登录",Public,&[];
    post "/open/v1/auth/refresh"=>refresh,"open.auth.refresh","openAuth","刷新应用用户会话",OpenRefreshCredential,&[];
    post "/open/v1/auth/logout"=>logout,"open.auth.logout","openAuth","注销应用用户会话",OpenRefreshCredential,&[];
    post "/open/v1/auth/application/logout"=>application_logout,"open.auth.applicationLogout","openAuth","注销应用服务会话",OpenApplication,&[];
    get "/open/v1/me"=>me,"open.profile.get","openProfile","应用用户本人资料",OpenUser,&[],scopes=&["open.profile.read"];
    patch "/open/v1/me"=>patch_me,"open.profile.update","openProfile","修改本人显示名称",OpenUser,&[],scopes=&["open.profile.write"];
}
pub fn configured_routes() -> axum::Router<AppState> {
    routes().layer(axum::middleware::from_fn(private))
}
async fn private(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}
fn peer(request: &Request) -> Option<std::net::IpAddr> {
    request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|v| v.0.ip())
}
async fn application(
    State(state): State<AppState>,
    request: Request,
) -> Result<ApiResponse<auth::Tokens>, AppError> {
    let peer = peer(&request);
    let input = request::json::<auth::ApplicationInput>(request, &state).await?;
    Ok(ApiResponse::success(
        auth::application(&state, input, peer).await?,
    ))
}
async fn login(
    State(state): State<AppState>,
    request: Request,
) -> Result<ApiResponse<auth::Tokens>, AppError> {
    let peer = peer(&request);
    let input = request::json::<auth::UserInput>(request, &state).await?;
    Ok(ApiResponse::success(
        auth::login(&state, input, peer).await?,
    ))
}
async fn refresh(
    State(state): State<AppState>,
    request: Request,
) -> Result<ApiResponse<auth::Tokens>, AppError> {
    let peer = peer(&request);
    let input = request::json::<auth::RefreshInput>(request, &state).await?;
    Ok(ApiResponse::success(
        auth::refresh(&state, input, peer).await?,
    ))
}
async fn logout(State(state): State<AppState>, request: Request) -> Result<StatusCode, AppError> {
    let input = request::json::<auth::RefreshInput>(request, &state).await?;
    auth::logout_user(&state, input).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn application_logout(
    State(state): State<AppState>,
    request: Request,
) -> Result<StatusCode, AppError> {
    let raw = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;
    auth::logout_application(&state, raw).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn me(
    State(state): State<AppState>,
    context: OpenContext,
) -> Result<ApiResponse<UserView>, AppError> {
    context.require(PrincipalKind::User, Scope::ProfileRead)?;
    Ok(ApiResponse::success(
        users::get(
            state.require_database()?,
            context.application_id(),
            context.session.user_id.ok_or(AppError::Unauthorized)?,
        )
        .await?,
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PatchMe {
    version: i64,
    display_name: String,
}
async fn patch_me(
    State(state): State<AppState>,
    context: OpenContext,
    request: Request,
) -> Result<ApiResponse<UserView>, AppError> {
    context.require(PrincipalKind::User, Scope::ProfileWrite)?;
    let input = request::json::<PatchMe>(request, &state).await?;
    crate::modules::applications::dto::version(input.version)?;
    let name = input.display_name.trim().to_owned();
    crate::modules::applications::dto::name(&name, "显示名称", 80)?;
    let database = state.require_database()?;
    let user=transaction::run(database,(database.clone(),context,name),|session,(database,context,name)|async move{let(record,_)=context::authorize(database,session,context,PrincipalKind::User,Scope::ProfileWrite).await?;let id=record.user_id.ok_or_else(||fail(Failure::Unauthorized))?;let changed=database.collection::<Document>("application_users").update_one(doc!{"_id":id,"applicationId":record.application_id,"version":input.version,"deletedAt":null,"status":"active"},doc!{"$set":{"displayName":name.as_str(),"updatedAt":DateTime::now()},"$inc":{"version":1i64}}).session(&mut *session).await?;if changed.matched_count!=1{return Err(fail(Failure::Conflict("用户版本已改变")));}super::sessions::audit(database,session,&record,"open.profile.update","success").await?;database.collection::<UserMeta>("application_users").find_one(doc!{"_id":id,"applicationId":record.application_id}).projection(crate::modules::applications::models::user_projection()).session(session).await?.ok_or_else(||fail(Failure::NotReady))}.boxed()).await?;
    Ok(ApiResponse::success(user.view()?))
}
