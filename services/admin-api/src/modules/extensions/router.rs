use super::{dto::*, models::*, service};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
        request,
        response::ApiResponse,
        validation,
    },
    modules::auth::AuthContext,
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
crate::api_routes! {
 prefix "";
 get "/api/v1/extensions"=>list,"skills.extensions.list","skills","扩展库列表",Permission,&["skills.read"];
 post "/api/v1/extensions"=>install,"skills.extensions.install","skills","新建或安装扩展",Permission,&["skills.read","skills.write"],"只保存包内容，不执行脚本；初始停用";
 get "/api/v1/extensions/by-request/{requestId}"=>by_request,"skills.extensions.byRequest","skills","按本人请求查询扩展变更",Permission,&["skills.read"];
 get "/api/v1/extensions/{id}"=>get,"skills.extensions.get","skills","扩展资料",Permission,&["skills.read"];
 patch "/api/v1/extensions/{id}"=>toggle,"skills.extensions.toggle","skills","启用或停用扩展",Permission,&["skills.read","skills.write"];
 delete "/api/v1/extensions/{id}"=>uninstall,"skills.extensions.uninstall","skills","卸载扩展",Permission,&["skills.read","skills.write"];
 get "/api/v1/extensions/{id}/revisions"=>history,"skills.revisions.list","skills","扩展修订历史",Permission,&["skills.read"];
 post "/api/v1/extensions/{id}/revisions"=>revise,"skills.revisions.create","skills","保存扩展新修订",Permission,&["skills.read","skills.write"],"保留原修订，新修订默认停用";
 get "/api/v1/extensions/{id}/revisions/{revisionId}"=>revision,"skills.revisions.get","skills","原修订资料",Permission,&["skills.read"];
 get "/api/v1/extensions/{id}/revisions/{revisionId}/preview"=>preview,"skills.revisions.preview","skills","预览原修订文本文件",Permission,&["skills.read"];
 get "/api/v1/extensions/{id}/revisions/{revisionId}/download"=>download,"skills.revisions.download","skills","下载原修订扩展包",Permission,&["skills.read"];
 get "/api/v1/skill-constraints"=>constraints,"skills.constraints.get","skills","读取全局 Skill 约束",Permission,&["skills.read"];
 patch "/api/v1/skill-constraints"=>update_constraints,"skills.constraints.update","skills","修改全局 Skill 约束",Permission,&["skills.constraints.write","skills.read"];
 get "/api/v1/skill-constraints/revisions"=>constraint_history,"skills.constraints.history","skills","全局 Skill 约束历史",Permission,&["skills.read"];
}
pub fn configured_routes() -> axum::Router<AppState> {
    routes()
        .layer(axum::extract::DefaultBodyLimit::max(6 * 1024 * 1024))
        .layer(axum::middleware::from_fn(private))
}
async fn private(request: Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
fn read(context: &AuthContext) -> Result<(), AppError> {
    context.require("skills.read")
}
fn write(context: &AuthContext) -> Result<(), AppError> {
    read(context)?;
    context.require("skills.write")
}
fn query<T>(input: Result<Query<T>, QueryRejection>) -> Result<T, AppError> {
    input
        .map(|Query(value)| value)
        .map_err(|_| AppError::Validation("扩展查询参数无效".into()))
}
fn created<T: serde::Serialize>(result: (T, bool)) -> (StatusCode, ApiResponse<T>) {
    (
        if result.1 {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(result.0),
    )
}
async fn list(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<LibraryQuery>, QueryRejection>,
) -> Result<Page<ExtensionView>, AppError> {
    read(&context)?;
    service::list(state.require_database()?, &context, query(input)?).await
}
async fn install(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ExtensionView>), AppError> {
    write(&context)?;
    let input = request::json::<Install>(body, &state).await?;
    Ok(created(
        service::install(state.require_database()?, &context, input).await?,
    ))
}
async fn by_request(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ExtensionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::by_request(state.require_database()?, &context, &id).await?,
    ))
}
async fn get(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ExtensionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::get(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn toggle(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<ExtensionView>, AppError> {
    write(&context)?;
    let input = request::json::<Toggle>(body, &state).await?;
    Ok(ApiResponse::success(
        service::toggle(&state, &context, validation::object_id(&id)?, input).await?,
    ))
}
async fn uninstall(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    write(&context)?;
    service::uninstall(
        state.require_database()?,
        &context,
        validation::object_id(&id)?,
        query(input)?.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn history(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<RevisionView>, AppError> {
    read(&context)?;
    service::history(
        state.require_database()?,
        &context,
        validation::object_id(&id)?,
        query(input)?,
    )
    .await
}
async fn revise(
    State(state): State<AppState>,
    context: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ExtensionView>), AppError> {
    write(&context)?;
    let input = request::json::<Revise>(body, &state).await?;
    Ok(created(
        service::revise(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn revision(
    State(state): State<AppState>,
    context: AuthContext,
    Path((id, revision)): Path<(String, String)>,
) -> Result<ApiResponse<RevisionView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::revision(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
            validation::object_id(&revision)?,
        )
        .await?,
    ))
}
async fn preview(
    State(state): State<AppState>,
    context: AuthContext,
    Path((id, revision)): Path<(String, String)>,
    input: Result<Query<Preview>, QueryRejection>,
) -> Result<ApiResponse<PreviewView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::preview(
            state.require_database()?,
            &context,
            validation::object_id(&id)?,
            validation::object_id(&revision)?,
            &query(input)?.path,
        )
        .await?,
    ))
}
async fn download(
    State(state): State<AppState>,
    context: AuthContext,
    Path((id, revision)): Path<(String, String)>,
) -> Result<Response, AppError> {
    read(&context)?;
    let id = validation::object_id(&id)?;
    let revision = validation::object_id(&revision)?;
    let (meta, package) =
        service::load_package(state.require_database()?, &context, id, revision).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"extension-{}-{}.zip\"",
                    id.to_hex(),
                    meta.number
                ),
            ),
            (header::CONTENT_LENGTH, package.archive.len().to_string()),
            (header::ETAG, format!("\"{}\"", package.archive_sha256)),
            (
                header::HeaderName::from_static("x-content-sha256"),
                package.archive_sha256,
            ),
        ],
        package.archive,
    )
        .into_response())
}
async fn constraints(
    State(state): State<AppState>,
    context: AuthContext,
) -> Result<ApiResponse<ConstraintView>, AppError> {
    read(&context)?;
    Ok(ApiResponse::success(
        service::constraints(state.require_database()?, &context).await?,
    ))
}
async fn update_constraints(
    State(state): State<AppState>,
    context: AuthContext,
    body: Request,
) -> Result<ApiResponse<ConstraintView>, AppError> {
    read(&context)?;
    context.require("skills.constraints.write")?;
    let input = request::json::<ConstraintInput>(body, &state).await?;
    Ok(ApiResponse::success(
        service::update_constraints(state.require_database()?, &context, input).await?,
    ))
}
async fn constraint_history(
    State(state): State<AppState>,
    context: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<ConstraintView>, AppError> {
    read(&context)?;
    service::constraint_history(state.require_database()?, &context, query(input)?).await
}
