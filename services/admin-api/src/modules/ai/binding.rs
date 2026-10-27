use super::models::Provider;
use crate::{
    common::{
        canonical,
        error::AppError,
        transaction::{Failure, fail},
    },
    config::Config,
    modules::settings::{models::Profile, service},
};
use mongodb::{
    ClientSession, Database,
    bson::{doc, oid::ObjectId},
};
pub fn definition(
    config: &Config,
    row: &Profile,
) -> Result<(String, reqwest::header::HeaderMap), AppError> {
    let (endpoint, headers) = service::http_definition(config, row)?;
    tri_ai_runtime::http::validate_definition(&endpoint, &headers)
        .map_err(|_| AppError::Validation("AI HTTP 端点或凭据头无效".into()))?;
    Ok((endpoint, headers))
}
fn fingerprint(config: &Config, row: &Profile, version: i64) -> Result<String, AppError> {
    if row.version != version {
        return Err(AppError::Conflict("HTTP 配置版本已改变，请重新绑定".into()));
    }
    let _ = definition(config, row)?;
    Ok(canonical::checksum(canonical::json(&serde_json::json!({"id":row.id.to_hex(),"version":row.version,"kind":row.kind,"configuration":row.configuration_json,"secret":row.secret_box,"enabled":row.enabled}))?.as_bytes()))
}
pub fn transaction_error(error: AppError) -> mongodb::error::Error {
    match error {
        AppError::SecretStoreUnavailable => fail(Failure::SecretStoreUnavailable),
        AppError::Conflict(_) => fail(Failure::Conflict("HTTP 配置版本已改变，请重新绑定")),
        AppError::Validation(_) => fail(Failure::Validation("AI HTTP 配置或凭据无效")),
        _ => fail(Failure::NotReady),
    }
}
pub async fn bind(
    db: &Database,
    session: &mut ClientSession,
    config: &Config,
    id: ObjectId,
    version: i64,
) -> mongodb::error::Result<String> {
    let row = db
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":id,"deletedAt":null})
        .session(session)
        .await?
        .ok_or_else(|| fail(Failure::Validation("HTTP 配置不存在")))?;
    fingerprint(config, &row, version).map_err(transaction_error)
}
pub async fn current(
    db: &Database,
    session: &mut ClientSession,
    config: &Config,
    provider: &Provider,
) -> mongodb::error::Result<bool> {
    let row = db
        .collection::<Profile>("integration_profiles")
        .find_one(doc! {"_id":provider.profile_id,"deletedAt":null})
        .session(session)
        .await?;
    Ok(row.is_some_and(|row| {
        fingerprint(config, &row, provider.profile_version)
            .is_ok_and(|hash| hash == provider.binding_fingerprint)
    }))
}
