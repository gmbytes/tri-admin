use super::{
    categories, chunks, cleanup,
    dto::{CategoryCreate, CategoryPatch, Empty, FileList, FilePatch, UploadCreate},
    jobs,
    models::{CHUNK_SIZE, CategoryView, FileView, UploadView},
    recovery, resources, uploads,
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
    body::to_bytes,
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::StatusCode,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteQuery {
    version: i64,
}
crate::api_routes! {
    prefix "";
    get "/api/v1/file-categories"=>category_list,"fileCategories.list","files","文件分类列表",Permission,&["files.read"];
    post "/api/v1/file-categories"=>category_create,"fileCategories.create","files","创建文件分类",Permission,&["files.write"];
    get "/api/v1/file-categories/{id}"=>category_read,"fileCategories.get","files","读取文件分类",Permission,&["files.read"];
    patch "/api/v1/file-categories/{id}"=>category_patch,"fileCategories.update","files","修改文件分类",Permission,&["files.write"];
    delete "/api/v1/file-categories/{id}"=>category_delete,"fileCategories.delete","files","删除文件分类",Permission,&["files.write"];
    get "/api/v1/uploads"=>upload_list,"uploads.list","files","我的上传会话",Permission,&["files.read"],"只返回当前操作者的上传会话";
    post "/api/v1/uploads"=>upload_create,"uploads.create","files","创建上传会话",Permission,&["files.write"],"requestId绑定操作者；同参数重放，异参数冲突";
    get "/api/v1/uploads/{id}"=>upload_read,"uploads.get","files","上传状态与分片清单",Permission,&["files.read"];
    put "/api/v1/uploads/{id}/chunks/{index}"=>chunk_put,"uploads.chunk.put","files","接收上传分片",Permission,&["files.write"],"octet-stream，需X-Chunk-Sha256；4MiB固定分片，重复同片不重复接收";
    post "/api/v1/uploads/{id}/cancel"=>upload_cancel,"uploads.cancel","files","取消上传",Permission,&["files.write"],"幂等持久取消，清理可能仍待确认";
    post "/api/v1/uploads/{id}/cleanup"=>upload_cleanup,"uploads.cleanup","files","重试上传清理",Permission,&["files.write"],"每批最多100个确定候选；仍在写入的尝试不宣称已清理";
    post "/api/v1/uploads/{id}/complete"=>upload_complete,"uploads.complete","files","校验并发布上传",Permission,&["files.write"],"持久排队与租约；202表示处理中，只有状态completed表示发布成功";
    post "/api/v1/uploads/{id}/recover"=>upload_recover,"uploads.recover","files","恢复过期分片",Permission,&["files.write"],"只处理创建者的过期或待确认候选；缺少即时内容不等于写入从未发生";
    post "/api/v1/uploads/{id}/retry"=>upload_retry,"uploads.retry","files","显式重试上传发布",Permission,&["files.write"],"先查询既有候选；缺少内容时使用新标识，并继续追踪旧写入";
    get "/api/v1/files"=>file_list,"files.list","files","文件资源列表",Permission,&["files.read"];
    get "/api/v1/files/{id}"=>file_read,"files.get","files","文件资料",Permission,&["files.read"];
    patch "/api/v1/files/{id}"=>file_patch,"files.update","files","修改文件资料",Permission,&["files.write"];
    delete "/api/v1/files/{id}"=>file_delete,"files.delete","files","删除文件资源",Permission,&["files.write"];
    get "/api/v1/files/{id}/content"=>file_content,"files.content","files","授权下载文件",Permission,&["files.read"],"按当前所有者范围授权，附件下载，正文流校验长度和SHA-256";
}
async fn upload_retry(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<UploadView>), AppError> {
    actor.require("files.write")?;
    request::json::<Empty>(body, &state).await?;
    let (pending, row) =
        jobs::retry(&state, Actor::new(&actor)?, validation::object_id(&id)?).await?;
    Ok((
        if pending {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
async fn upload_recover(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<recovery::RecoveryView>, AppError> {
    actor.require("files.write")?;
    request::json::<Empty>(body, &state).await?;
    Ok(ApiResponse::success(
        recovery::recover(&state, Actor::new(&actor)?.id, validation::object_id(&id)?).await?,
    ))
}
async fn upload_complete(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<UploadView>), AppError> {
    actor.require("files.write")?;
    request::json::<Empty>(body, &state).await?;
    let (pending, row) =
        jobs::enqueue(&state, Actor::new(&actor)?, validation::object_id(&id)?).await?;
    Ok((
        if pending {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        ApiResponse::success(row),
    ))
}
async fn file_list(
    State(state): State<AppState>,
    actor: AuthContext,
    input: Result<Query<FileList>, QueryRejection>,
) -> Result<Page<FileView>, AppError> {
    actor.require("files.read")?;
    let Query(input) = input.map_err(|_| AppError::Validation("文件查询无效".into()))?;
    resources::list(state.require_database()?, &Actor::new(&actor)?, input).await
}
async fn file_read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<FileView>, AppError> {
    actor.require("files.read")?;
    Ok(ApiResponse::success(
        resources::get(
            state.require_database()?,
            &Actor::new(&actor)?,
            validation::object_id(&id)?,
        )
        .await?
        .view()?,
    ))
}
async fn file_patch(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<FileView>, AppError> {
    actor.require("files.write")?;
    let input = request::json::<FilePatch>(body, &state).await?;
    Ok(ApiResponse::success(
        resources::patch(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn file_delete(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    actor.require("files.write")?;
    let Query(input) = input.map_err(|_| AppError::Validation("文件版本参数无效".into()))?;
    resources::delete(
        &state,
        Actor::new(&actor)?,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn file_content(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<axum::response::Response, AppError> {
    actor.require("files.read")?;
    resources::content(&state, &Actor::new(&actor)?, validation::object_id(&id)?).await
}
async fn category_list(
    State(state): State<AppState>,
    actor: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<CategoryView>, AppError> {
    actor.require("files.read")?;
    let Query(input) = input.map_err(|_| AppError::Validation("分类查询无效".into()))?;
    categories::list(state.require_database()?, input).await
}
async fn category_read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<CategoryView>, AppError> {
    actor.require("files.read")?;
    Ok(ApiResponse::success(
        categories::get(state.require_database()?, validation::object_id(&id)?).await?,
    ))
}
async fn category_create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<CategoryView>), AppError> {
    actor.require("files.write")?;
    let input = request::json::<CategoryCreate>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            categories::create(state.require_database()?, Actor::new(&actor)?, input).await?,
        ),
    ))
}
async fn category_patch(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<CategoryView>, AppError> {
    actor.require("files.write")?;
    let input = request::json::<CategoryPatch>(body, &state).await?;
    Ok(ApiResponse::success(
        categories::patch(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn category_delete(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    actor.require("files.write")?;
    let Query(input) = input.map_err(|_| AppError::Validation("分类版本参数无效".into()))?;
    categories::delete(
        state.require_database()?,
        Actor::new(&actor)?,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn upload_list(
    State(state): State<AppState>,
    actor: AuthContext,
    input: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<UploadView>, AppError> {
    actor.require("files.read")?;
    let Query(input) = input.map_err(|_| AppError::Validation("上传查询无效".into()))?;
    uploads::list(state.require_database()?, &Actor::new(&actor)?, input).await
}
async fn upload_read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<UploadView>, AppError> {
    actor.require("files.read")?;
    Ok(ApiResponse::success(
        uploads::get(
            state.require_database()?,
            Actor::new(&actor)?.id,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn upload_create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<UploadView>), AppError> {
    actor.require("files.write")?;
    let input = request::json::<UploadCreate>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            uploads::create(
                state.require_database()?,
                &state.config,
                Actor::new(&actor)?,
                input,
            )
            .await?,
        ),
    ))
}
async fn upload_cancel(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<UploadView>, AppError> {
    actor.require("files.write")?;
    request::json::<Empty>(body, &state).await?;
    Ok(ApiResponse::success(
        uploads::cancel(&state, Actor::new(&actor)?, validation::object_id(&id)?).await?,
    ))
}
async fn upload_cleanup(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<cleanup::CleanupView>, AppError> {
    actor.require("files.write")?;
    request::json::<Empty>(body, &state).await?;
    let owner = Actor::new(&actor)?.id;
    let id = validation::object_id(&id)?;
    let result = cleanup::run(&state, owner, id).await?;
    jobs::recheck(&state, owner, id).await?;
    Ok(ApiResponse::success(result))
}
async fn chunk_put(
    State(state): State<AppState>,
    actor: AuthContext,
    Path((id, index)): Path<(String, String)>,
    body: Request,
) -> Result<ApiResponse<chunks::Receipt>, AppError> {
    actor.require("files.write")?;
    let actor = Actor::new(&actor)?;
    let id = validation::object_id(&id)?;
    let index = index
        .parse::<i64>()
        .map_err(|_| AppError::Validation("分片索引无效".into()))?;
    chunks::preflight(state.require_database()?, actor.id, id, index).await?;
    let _slot = state
        .file_transfer_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let content_type = body
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok())
        .ok_or(AppError::UnsupportedMediaType)?;
    if content_type.type_() != mime::APPLICATION || content_type.subtype() != mime::OCTET_STREAM {
        return Err(AppError::UnsupportedMediaType);
    }
    let hash = body
        .headers()
        .get("x-chunk-sha256")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Validation("缺少X-Chunk-Sha256".into()))?;
    let hash = super::validate::spec(0, hash)?.sha256().to_owned();
    if body
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size > CHUNK_SIZE)
    {
        return Err(AppError::PayloadTooLarge);
    }
    let data = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        to_bytes(body.into_body(), CHUNK_SIZE as usize),
    )
    .await
    .map_err(|_| AppError::BadRequest)?
    .map_err(|_| AppError::PayloadTooLarge)?;
    if format!("{:x}", Sha256::digest(&data)) != hash {
        return Err(AppError::Validation("分片SHA-256不匹配".into()));
    }
    Ok(ApiResponse::success(
        chunks::put(&state, actor, id, index, data, hash).await?,
    ))
}
