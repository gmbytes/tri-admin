use crate::{
    common::{api::Endpoint, error::AppError, response::ApiResponse},
    modules::auth::AuthContext,
};
use axum::extract::{Query, rejection::QueryRejection};
use serde::{Deserialize, Serialize};

crate::api_routes! {
    prefix "";
    get "/api/v1/api-catalog" => list, "apiCatalog.list", "permissions", "查看 API 目录", Permission, &["permissions.read"];
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoQuery {}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    schema_version: u32,
    checksum: String,
    endpoints: Vec<Endpoint>,
}

pub fn endpoints() -> Vec<Endpoint> {
    let mut entries = super::registry()
        .into_iter()
        .flat_map(|module| module.endpoints.iter().copied())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.operation_id);
    entries
}
pub fn manifest() -> Result<Catalog, AppError> {
    let entries = endpoints();
    let json = crate::common::canonical::json(&entries)?;
    Ok(Catalog {
        schema_version: 1,
        checksum: crate::common::canonical::checksum(json.as_bytes()),
        endpoints: entries,
    })
}
async fn list(
    actor: AuthContext,
    query: Result<Query<NoQuery>, QueryRejection>,
) -> Result<ApiResponse<Catalog>, AppError> {
    actor.require("permissions.read")?;
    query.map_err(|_| AppError::Validation("API 目录不接受查询参数".into()))?;
    Ok(ApiResponse::success(manifest()?))
}
