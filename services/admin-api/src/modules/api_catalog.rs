use crate::{
    common::{api::Endpoint, error::AppError, response::ApiResponse},
    modules::auth::AuthContext,
};
use axum::extract::{Query, rejection::QueryRejection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    let value = serde_json::to_value(&entries).map_err(|_| AppError::Internal)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| AppError::Internal)?;
    Ok(Catalog {
        schema_version: 1,
        checksum: format!("{:x}", Sha256::digest(bytes)),
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
