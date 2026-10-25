use super::{
    dto::*,
    models::{ProfileView, ServiceView},
    service,
};
use crate::{
    common::{error::AppError, pagination::Page, request, response::ApiResponse, validation},
    modules::{auth::AuthContext, rbac::shared::Actor},
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State, rejection::QueryRejection},
    http::StatusCode,
};
use serde_json::{Value, json};
crate::api_routes! {
 prefix "";
 get "/api/v1/application" => application, "application.get", "settings", "应用显示信息", Authenticated, &[];
 get "/api/v1/settings/runtime" => runtime, "settings.runtime", "settings", "安全启动配置", Permission, &["settings.read"];
 get "/api/v1/settings/service" => service_get, "settings.service.get", "settings", "服务在线配置", Permission, &["settings.read"];
 patch "/api/v1/settings/service" => service_patch, "settings.service.update", "settings", "修改服务在线配置", Permission, &["settings.write"];
 get "/api/v1/settings/profiles" => list, "settings.profiles.list", "settings", "集成配置列表", Permission, &["settings.read"];
 post "/api/v1/settings/profiles" => create, "settings.profiles.create", "settings", "新建集成配置", Permission, &["settings.write"];
 get "/api/v1/settings/profiles/{id}" => read, "settings.profiles.get", "settings", "集成配置详情", Permission, &["settings.read"];
 patch "/api/v1/settings/profiles/{id}" => patch, "settings.profiles.update", "settings", "修改集成配置", Permission, &["settings.write"];
 delete "/api/v1/settings/profiles/{id}" => delete, "settings.profiles.delete", "settings", "删除集成配置", Permission, &["settings.write"];
 post "/api/v1/settings/profiles/{id}/rewrap" => rewrap, "settings.profiles.rewrap", "settings", "轮换配置加密密钥", Permission, &["settings.write"];
 post "/api/v1/settings/profiles/{id}/probe" => probe, "settings.profiles.probe", "settings", "检查集成连接", Permission, &["settings.write"], "使用保存的配置与凭据；有界连接/认证检查，不发送邮件";
 get "/api/v1/settings/profiles/{id}/checks" => checks, "settings.profiles.checks", "settings", "连接检查历史", Permission, &["settings.read"];
}
async fn application(
    State(state): State<AppState>,
    _actor: AuthContext,
) -> Result<ApiResponse<Value>, AppError> {
    let row = service::get_service(state.require_database()?).await?;
    Ok(ApiResponse::success(
        json!({"displayName":row.display_name,"version":env!("CARGO_PKG_VERSION")}),
    ))
}
async fn probe(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<super::check_model::CheckView>, AppError> {
    actor.require("settings.write")?;
    let input = request::json::<VersionInput>(body, &state).await?;
    Ok(ApiResponse::success(
        super::probe::execute(
            &state,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input.version,
        )
        .await?,
    ))
}
async fn checks(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    input: Result<Query<crate::common::pagination::PageInput>, QueryRejection>,
) -> Result<Page<super::check_model::CheckView>, AppError> {
    actor.require("settings.read")?;
    let Query(input) = input.map_err(|_| AppError::Validation("检查记录查询无效".into()))?;
    super::probe::history(&state, validation::object_id(&id)?, input).await
}
async fn runtime(
    State(state): State<AppState>,
    actor: AuthContext,
) -> Result<ApiResponse<Value>, AppError> {
    actor.require("settings.read")?;
    let config = &state.config;
    Ok(ApiResponse::success(
        json!({"configuredBind":config.bind.to_string(),"databaseName":state.database.as_ref().map(|database|database.name()),"corsOrigins":config.cors_origins.iter().filter_map(|value|value.to_str().ok()).collect::<Vec<_>>(),"storageRoot":config.storage_root,"accessTokenSeconds":900,"sessionDays":7,"bootLoginLimitPerIp":config.login_limit_per_ip,"databaseConfigured":state.database.is_some(),"settingsKeysConfigured":config.settings_active_key().is_some(),"settingsActiveKeyId":config.settings_active_key(),"settingsDecryptionKeyIds":config.settings_key_ids()}),
    ))
}
async fn service_get(
    State(state): State<AppState>,
    actor: AuthContext,
) -> Result<ApiResponse<ServiceView>, AppError> {
    actor.require("settings.read")?;
    Ok(ApiResponse::success(
        service::service_view(state.require_database()?, &state.config).await?,
    ))
}
async fn service_patch(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<ApiResponse<ServiceView>, AppError> {
    actor.require("settings.write")?;
    let input = request::json::<ServicePatch>(body, &state).await?;
    Ok(ApiResponse::success(
        service::patch_service(
            state.require_database()?,
            &state.config,
            Actor::new(&actor)?,
            input,
        )
        .await?,
    ))
}
async fn list(
    State(state): State<AppState>,
    actor: AuthContext,
    input: Result<Query<ListInput>, QueryRejection>,
) -> Result<Page<ProfileView>, AppError> {
    actor.require("settings.read")?;
    let Query(input) = input.map_err(|_| AppError::Validation("集成配置查询无效".into()))?;
    service::list(state.require_database()?, &state.config, input).await
}
async fn create(
    State(state): State<AppState>,
    actor: AuthContext,
    body: Request,
) -> Result<(StatusCode, ApiResponse<ProfileView>), AppError> {
    actor.require("settings.write")?;
    let input = request::json::<Create>(body, &state).await?;
    Ok((
        StatusCode::CREATED,
        ApiResponse::success(
            service::create(
                state.require_database()?,
                &state.config,
                Actor::new(&actor)?,
                input,
            )
            .await?,
        ),
    ))
}
async fn read(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
) -> Result<ApiResponse<ProfileView>, AppError> {
    actor.require("settings.read")?;
    Ok(ApiResponse::success(
        service::get(
            state.require_database()?,
            &state.config,
            validation::object_id(&id)?,
        )
        .await?,
    ))
}
async fn patch(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<ProfileView>, AppError> {
    actor.require("settings.write")?;
    let input = request::json::<Patch>(body, &state).await?;
    Ok(ApiResponse::success(
        service::patch(
            state.require_database()?,
            &state.config,
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
    input: Result<Query<VersionInput>, QueryRejection>,
) -> Result<StatusCode, AppError> {
    actor.require("settings.write")?;
    let Query(input) = input.map_err(|_| AppError::Validation("版本参数无效".into()))?;
    service::delete(
        state.require_database()?,
        Actor::new(&actor)?,
        validation::object_id(&id)?,
        input.version,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn rewrap(
    State(state): State<AppState>,
    actor: AuthContext,
    Path(id): Path<String>,
    body: Request,
) -> Result<ApiResponse<ProfileView>, AppError> {
    actor.require("settings.write")?;
    let input = request::json::<VersionInput>(body, &state).await?;
    Ok(ApiResponse::success(
        service::rewrap(
            state.require_database()?,
            &state.config,
            Actor::new(&actor)?,
            validation::object_id(&id)?,
            input.version,
        )
        .await?,
    ))
}
