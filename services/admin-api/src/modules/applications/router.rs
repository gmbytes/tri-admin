use super::{
    dto::{
        CreateApplication, CreateUser, DeleteQuery, PasswordInput, PatchApplication, PatchUser,
        RotateSecret,
    },
    models::{ActionView, ApplicationView, IssuedApplication, RotatedApplication, UserView},
    service, users,
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        request,
        response::ApiResponse,
        validation,
    },
    modules::{auth::AuthContext, rbac::shared::Actor},
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::{HeaderValue, StatusCode, header::CACHE_CONTROL},
    middleware::Next,
    response::Response,
};
crate::api_routes! {
    prefix "";
    get "/api/v1/applications"=>list,"applications.list","applications","开放应用列表",Permission,&["applications.read"];
    post "/api/v1/applications"=>create,"applications.create","applications","创建开放应用",Permission,&["applications.write"],"仅交互会话；Scope 需相应应用用户权限，密钥仅首次展示";
    get "/api/v1/applications/by-request/{requestId}"=>by_request,"applications.byRequest","applications","按请求查询本人创建的应用",Permission,&["applications.read"];
    get "/api/v1/applications/{id}"=>get,"applications.get","applications","开放应用资料",Permission,&["applications.read"];
    patch "/api/v1/applications/{id}"=>patch,"applications.update","applications","修改开放应用",Permission,&["applications.write"],"仅交互会话；策略授权不得超过当前权限";
    delete "/api/v1/applications/{id}"=>delete,"applications.delete","applications","删除开放应用",Permission,&["applications.write"],"仅交互会话";
    post "/api/v1/applications/{id}/secret"=>rotate,"applications.rotateSecret","applications","轮换应用密钥",Permission,&["applications.write"],"仅交互会话；Scope 需相应应用用户权限";
    get "/api/v1/applications/{id}/actions/{requestId}"=>action,"applications.action","applications","查询本人密钥轮换回执",Permission,&["applications.read"];
    get "/api/v1/applications/{application}/users"=>list_users,"applicationUsers.list","applicationUsers","应用用户列表",Permission,&["applicationUsers.read","applications.read"];
    post "/api/v1/applications/{application}/users"=>create_user,"applicationUsers.create","applicationUsers","创建应用用户",Permission,&["applicationUsers.read","applicationUsers.write","applications.read"],"仅交互会话；用户 Scope 只能是应用允许的 profile Scope";
    get "/api/v1/applications/{application}/users/{id}"=>get_user,"applicationUsers.get","applicationUsers","应用用户资料",Permission,&["applicationUsers.read","applications.read"];
    patch "/api/v1/applications/{application}/users/{id}"=>patch_user,"applicationUsers.update","applicationUsers","修改应用用户",Permission,&["applicationUsers.read","applicationUsers.write","applications.read"],"仅交互会话";
    delete "/api/v1/applications/{application}/users/{id}"=>delete_user,"applicationUsers.delete","applicationUsers","删除应用用户",Permission,&["applicationUsers.read","applicationUsers.write","applications.read"],"仅交互会话";
    post "/api/v1/applications/{application}/users/{id}/password"=>password,"applicationUsers.resetPassword","applicationUsers","重置应用用户密码",Permission,&["applicationUsers.read","applicationUsers.write","applications.read"],"仅交互会话";
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
fn read_users(context: &AuthContext) -> Result<(), AppError> {
    context.require("applications.read")?;
    context.require("applicationUsers.read")
}
fn write_users(context: &AuthContext) -> Result<(), AppError> {
    read_users(context)?;
    context.require("applicationUsers.write")?;
    context.require_session()?;
    Ok(())
}
fn write(context: &AuthContext) -> Result<(), AppError> {
    context.require("applications.write")?;
    context.require_session()?;
    Ok(())
}
async fn list(
    State(state): State<AppState>,
    context: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<ApplicationView>, AppError> {
    context.require("applications.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("应用查询参数无效".into()))?;
    service::list(state.require_database()?, input).await
}
async fn create(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<IssuedApplication>), AppError> {
    write(&context)?;
    let input = request::json::<CreateApplication>(body, &state).await?;
    let (view, created) = service::create(state.require_database()?, &context, input).await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(view),
    ))
}
async fn get(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ApplicationView>, AppError> {
    context.require("applications.read")?;
    Ok(ApiResponse::success(
        service::get(state.require_database()?, validation::object_id(&id)?).await?,
    ))
}
async fn by_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(request): Path<String>,
) -> Result<ApiResponse<ApplicationView>, AppError> {
    context.require("applications.read")?;
    Ok(ApiResponse::success(
        service::by_request(
            state.require_database()?,
            Actor::new(&context)?.id,
            &request,
        )
        .await?,
    ))
}
async fn patch(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<ApplicationView>, AppError> {
    write(&context)?;
    let input = request::json::<PatchApplication>(body, &state).await?;
    Ok(ApiResponse::success(
        service::patch(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn delete(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    write(&context)?;
    let Query(input) = query.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    service::delete(
        state.require_database()?,
        &context,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn rotate(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<RotatedApplication>, AppError> {
    write(&context)?;
    let input = request::json::<RotateSecret>(body, &state).await?;
    Ok(ApiResponse::success(
        service::rotate(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn action(
    State(state): State<AppState>,
    context: AuthContext,
    Path((id, request)): Path<(String, String)>,
) -> Result<ApiResponse<ActionView>, AppError> {
    context.require("applications.read")?;
    Ok(ApiResponse::success(
        service::action(
            state.require_database()?,
            validation::object_id(&id)?,
            Actor::new(&context)?.id,
            &request,
        )
        .await?,
    ))
}
async fn list_users(
    State(state): State<AppState>,
    context: AuthContext,
    Path(application): Path<String>,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<UserView>, AppError> {
    read_users(&context)?;
    let Query(input) = query.map_err(|_| AppError::Validation("应用用户查询无效".into()))?;
    users::list(
        state.require_database()?,
        validation::object_id(&application)?,
        input,
    )
    .await
}
async fn create_user(
    State(state): State<AppState>,
    context: AuthContext,
    Path(application): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<UserView>), AppError> {
    write_users(&context)?;
    let input = request::json::<CreateUser>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            users::create(
                state.require_database()?,
                &context,
                validation::object_id(&application)?,
                input,
            )
            .await?,
        ),
    ))
}
async fn get_user(
    State(state): State<AppState>,
    context: AuthContext,
    Path((application, id)): Path<(String, String)>,
) -> Result<ApiResponse<UserView>, AppError> {
    read_users(&context)?;
    Ok(ApiResponse::success(
        users::get(
            state.require_database()?,
            validation::object_id(&application)?,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn patch_user(
    State(state): State<AppState>,
    context: AuthContext,
    Path((application, id)): Path<(String, String)>,
    body: Request,
) -> Result<ApiResponse<UserView>, AppError> {
    write_users(&context)?;
    let input = request::json::<PatchUser>(body, &state).await?;
    Ok(ApiResponse::success(
        users::patch(
            state.require_database()?,
            &context,
            validation::object_id(&application)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn delete_user(
    State(state): State<AppState>,
    context: AuthContext,
    Path((application, id)): Path<(String, String)>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    write_users(&context)?;
    let Query(input) = query.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    users::delete(
        state.require_database()?,
        &context,
        validation::object_id(&application)?,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn password(
    State(state): State<AppState>,
    context: AuthContext,
    Path((application, id)): Path<(String, String)>,
    body: Request,
) -> Result<StatusCode, AppError> {
    write_users(&context)?;
    let input = request::json::<PasswordInput>(body, &state).await?;
    users::reset_password(
        state.require_database()?,
        &context,
        validation::object_id(&application)?,
        validation::object_id(&id)?,
        input,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
