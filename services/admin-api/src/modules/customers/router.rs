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
    Router,
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::StatusCode,
    routing::get,
};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteQuery {
    version: i64,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/customers", get(list).post(create))
        .route("/api/v1/customers/owners", get(owners))
        .route(
            "/api/v1/customers/{id}",
            get(read).patch(update).delete(delete),
        )
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
