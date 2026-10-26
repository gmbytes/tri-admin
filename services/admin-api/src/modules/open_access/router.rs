use super::{
    auth,
    context::{self, OpenContext},
    scopes::Scope,
    sessions::PrincipalKind,
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        request,
        response::ApiResponse,
        transaction::{self, Failure, fail},
        validation,
    },
    modules::applications::{
        dto::{CreateUser, DeleteQuery, PasswordInput, PatchUser},
        models::{UserMeta, UserView},
        user_authority::Authority,
        users,
    },
    state::AppState,
};
use axum::{
    extract::{ConnectInfo, Path, Query, Request, State, rejection::QueryRejection},
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
    get "/open/v1/users"=>list_users,"open.users.list","openUsers","应用服务用户列表",OpenApplication,&[],scopes=&["open.users.read"];
    post "/open/v1/users"=>create_user,"open.users.create","openUsers","应用服务创建用户",OpenApplication,&[],scopes=&["open.users.write"],"授予 profile Scope 不得超过本令牌与应用集合";
    get "/open/v1/users/{id}"=>get_user,"open.users.get","openUsers","应用服务用户资料",OpenApplication,&[],scopes=&["open.users.read"];
    patch "/open/v1/users/{id}"=>update_user,"open.users.update","openUsers","应用服务修改用户",OpenApplication,&[],scopes=&["open.users.write"];
    delete "/open/v1/users/{id}"=>delete_user,"open.users.delete","openUsers","应用服务删除用户",OpenApplication,&[],scopes=&["open.users.write"];
    post "/open/v1/users/{id}/password"=>reset_password,"open.users.resetPassword","openUsers","应用服务重置密码",OpenApplication,&[],scopes=&["open.users.write"],"凭据授予不得超过本令牌的 profile Scope";
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
async fn list_users(
    State(state): State<AppState>,
    context: OpenContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<UserView>, AppError> {
    context.require(PrincipalKind::Application, Scope::UsersRead)?;
    let Query(input) = query.map_err(|_| AppError::Validation("用户查询参数无效".into()))?;
    users::list(state.require_database()?, context.application_id(), input).await
}
async fn get_user(
    State(state): State<AppState>,
    context: OpenContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<UserView>, AppError> {
    context.require(PrincipalKind::Application, Scope::UsersRead)?;
    Ok(ApiResponse::success(
        users::get(
            state.require_database()?,
            context.application_id(),
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn create_user(
    State(state): State<AppState>,
    context: OpenContext,
    request: Request,
) -> Result<(StatusCode, ApiResponse<UserView>), AppError> {
    let authority = Authority::application(&context)?;
    let input = request::json::<CreateUser>(request, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            users::create(
                state.require_database()?,
                authority,
                context.application_id(),
                input,
            )
            .await?,
        ),
    ))
}
async fn update_user(
    State(state): State<AppState>,
    context: OpenContext,
    Path(id): Path<String>,
    request: Request,
) -> Result<ApiResponse<UserView>, AppError> {
    let authority = Authority::application(&context)?;
    let input = request::json::<PatchUser>(request, &state).await?;
    Ok(ApiResponse::success(
        users::patch(
            state.require_database()?,
            authority,
            context.application_id(),
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn delete_user(
    State(state): State<AppState>,
    context: OpenContext,
    Path(id): Path<String>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    let authority = Authority::application(&context)?;
    let Query(input) = query.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    users::delete(
        state.require_database()?,
        authority,
        context.application_id(),
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn reset_password(
    State(state): State<AppState>,
    context: OpenContext,
    Path(id): Path<String>,
    request: Request,
) -> Result<StatusCode, AppError> {
    let authority = Authority::application(&context)?;
    let input = request::json::<PasswordInput>(request, &state).await?;
    users::reset_password(
        state.require_database()?,
        authority,
        context.application_id(),
        validation::object_id(&id)?,
        input,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
