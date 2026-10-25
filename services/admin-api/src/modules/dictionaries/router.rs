use super::{
    dto::{CreateDictionary, CreateItem, PatchDictionary, PatchItem},
    models::{DictionaryView, ItemView},
    service,
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
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteQuery {
    version: i64,
}

crate::api_routes! {
    prefix "";
    get "/api/v1/dictionaries" => list, "dictionaries.list", "dictionaries", "字典列表", Permission, &["dictionaries.read"];
    post "/api/v1/dictionaries" => create, "dictionaries.create", "dictionaries", "创建字典", Permission, &["dictionaries.write"];
    get "/api/v1/dictionaries/{id}" => read, "dictionaries.get", "dictionaries", "字典资料", Permission, &["dictionaries.read"];
    patch "/api/v1/dictionaries/{id}" => update, "dictionaries.update", "dictionaries", "修改字典", Permission, &["dictionaries.write"];
    delete "/api/v1/dictionaries/{id}" => delete, "dictionaries.delete", "dictionaries", "删除字典", Permission, &["dictionaries.write"];
    get "/api/v1/dictionaries/{id}/items" => list_items, "dictionaries.items.list", "dictionaries", "字典明细列表", Permission, &["dictionaries.read"];
    post "/api/v1/dictionaries/{id}/items" => create_item, "dictionaries.items.create", "dictionaries", "创建字典明细", Permission, &["dictionaries.write"];
    patch "/api/v1/dictionaries/{id}/items/{item_id}" => update_item, "dictionaries.items.update", "dictionaries", "修改字典明细", Permission, &["dictionaries.write"];
    delete "/api/v1/dictionaries/{id}/items/{item_id}" => delete_item, "dictionaries.items.delete", "dictionaries", "删除字典明细", Permission, &["dictionaries.write"];
}
async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<DictionaryView>, AppError> {
    actor.require("dictionaries.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::list(state.require_database()?, input).await
}
async fn read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<DictionaryView>, AppError> {
    actor.require("dictionaries.read")?;
    Ok(ApiResponse::success(
        service::get(state.require_database()?, validation::object_id(&id)?).await?,
    ))
}
async fn create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<DictionaryView>), AppError> {
    actor.require("dictionaries.write")?;
    let input = request::json::<CreateDictionary>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            service::create(state.require_database()?, Actor::new(&actor)?, input).await?,
        ),
    ))
}
async fn update(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<DictionaryView>, AppError> {
    actor.require("dictionaries.write")?;
    let input = request::json::<PatchDictionary>(body, &state).await?;
    Ok(ApiResponse::success(
        service::patch(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input,
        )
        .await?,
    ))
}
async fn delete(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    actor.require("dictionaries.write")?;
    let Query(input) = query.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    service::delete(
        state.require_database()?,
        Actor::new(&actor)?,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn list_items(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<ItemView>, AppError> {
    actor.require("dictionaries.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::list_items(
        state.require_database()?,
        validation::object_id(&id)?,
        input,
    )
    .await
}
async fn create_item(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ItemView>), AppError> {
    actor.require("dictionaries.write")?;
    let input = request::json::<CreateItem>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            service::create_item(
                state.require_database()?,
                Actor::new(&actor)?,
                validation::object_id(&id)?,
                input,
            )
            .await?,
        ),
    ))
}
async fn update_item(
    State(state): State<AppState>,
    actor: AuthContext,
    Path((id, item_id)): Path<(String, String)>,
    body: Request,
) -> Result<ApiResponse<ItemView>, AppError> {
    actor.require("dictionaries.write")?;
    let input = request::json::<PatchItem>(body, &state).await?;
    Ok(ApiResponse::success(
        service::patch_item(
            state.require_database()?,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            validation::object_id(&item_id)?,
            input,
        )
        .await?,
    ))
}
async fn delete_item(
    State(state): State<AppState>,
    actor: AuthContext,
    Path((id, item_id)): Path<(String, String)>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    actor.require("dictionaries.write")?;
    let Query(input) = query.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    service::delete_item(
        state.require_database()?,
        Actor::new(&actor)?,
        validation::object_id(&id)?,
        validation::object_id(&item_id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
