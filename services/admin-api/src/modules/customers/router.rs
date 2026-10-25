use super::{
    dto::{CreateCustomer, CustomerQuery, PatchCustomer},
    models::{CustomerView, OwnerView},
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
    get "/api/v1/customers" => list, "customers.list", "customers", "客户列表", Permission, &["customers.read"];
    post "/api/v1/customers" => create, "customers.create", "customers", "创建客户", Permission, &["customers.write"];
    get "/api/v1/customers/{id}" => read, "customers.get", "customers", "客户资料", Permission, &["customers.read"];
    patch "/api/v1/customers/{id}" => update, "customers.update", "customers", "修改客户归属或资料", Permission, &["customers.write"];
    delete "/api/v1/customers/{id}" => delete, "customers.delete", "customers", "删除客户", Permission, &["customers.write"];
    get "/api/v1/customers/owners" => owners, "customers.owners", "customers", "可选择归属人", Permission, &["customers.write"];
}
async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<CustomerQuery>, QueryRejection>,
) -> Result<Page<CustomerView>, AppError> {
    actor.require("customers.read")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::list(state.require_database()?, &Actor::new(&actor)?, input).await
}
async fn owners(
    State(state): State<AppState>,
    actor: AuthContext,
    query: Result<Query<PageInput>, QueryRejection>,
) -> Result<Page<OwnerView>, AppError> {
    actor.require("customers.write")?;
    let Query(input) = query.map_err(|_| AppError::Validation("查询参数无效".into()))?;
    service::owners(state.require_database()?, &Actor::new(&actor)?, input).await
}
async fn read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<CustomerView>, AppError> {
    actor.require("customers.read")?;
    Ok(ApiResponse::success(
        service::get(
            state.require_database()?,
            &Actor::new(&actor)?,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<CustomerView>), AppError> {
    actor.require("customers.write")?;
    let input = request::json::<CreateCustomer>(body, &state).await?;
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
) -> Result<ApiResponse<CustomerView>, AppError> {
    actor.require("customers.write")?;
    let input = request::json::<PatchCustomer>(body, &state).await?;
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
    actor.require("customers.write")?;
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
