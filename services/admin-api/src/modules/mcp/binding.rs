use super::{capabilities::Capability, dto::Transport, models::Connection};
use crate::{
    common::{
        canonical,
        error::AppError,
        transaction::{Failure, fail},
        validation,
    },
    config::Config,
    modules::{
        rbac::shared::{self, Actor},
        settings::{models::Profile, service},
    },
    state::AppState,
};
use mongodb::{
    ClientSession, Database,
    bson::{doc, oid::ObjectId},
};
use serde_json::json;
use std::collections::BTreeSet;
use tri_mcp_runtime::session::Limits;

#[derive(Clone)]
pub(super) enum Binding {
    Stdio(Capability),
    Http(HttpBinding),
}
#[derive(Clone)]
pub(super) struct HttpBinding {
    pub profile_id: ObjectId,
    pub transport: Transport,
    pub fingerprint: String,
}
impl Binding {
    pub fn fingerprint(&self) -> String {
        match self {
            Self::Stdio(v) => v.fingerprint(),
            Self::Http(v) => v.fingerprint.clone(),
        }
    }
    pub fn allowed_tools(&self) -> &[String] {
        match self {
            Self::Stdio(v) => &v.allowed_tools,
            Self::Http(v) => match &v.transport {
                Transport::Http { allowed_tools, .. } => allowed_tools,
                _ => unreachable!(),
            },
        }
    }
}
fn error(value: AppError) -> mongodb::error::Error {
    match value {
        AppError::SecretStoreUnavailable => fail(Failure::SecretStoreUnavailable),
        AppError::Validation(_) => fail(Failure::Validation("HTTP MCP 配置或引用无效")),
        AppError::Conflict(_) => fail(Failure::Conflict("HTTP 配置版本已改变，请重新绑定")),
        _ => fail(Failure::NotReady),
    }
}
fn reference(transport: &Transport) -> Result<(ObjectId, i64), AppError> {
    let Transport::Http {
        profile_id,
        profile_version,
        allowed_tools,
    } = transport
    else {
        return Err(AppError::Internal);
    };
    let id = validation::object_id(profile_id)?;
    super::dto::version(*profile_version)?;
    if id.to_hex() != *profile_id
        || allowed_tools.is_empty()
        || allowed_tools.len() > 128
        || allowed_tools.iter().any(|value| {
            value.trim().is_empty()
                || value.chars().count() > 128
                || value.chars().any(char::is_control)
        })
        || allowed_tools.iter().collect::<BTreeSet<_>>().len() != allowed_tools.len()
        || serde_json::to_vec(allowed_tools).map_or(true, |v| v.len() > 16 * 1024)
    {
        return Err(AppError::Validation("HTTP MCP 工具白名单无效".into()));
    }
    Ok((id, *profile_version))
}
fn http(config: &Config, transport: &Transport, row: &Profile) -> Result<HttpBinding, AppError> {
    let (id, version) = reference(transport)?;
    if row.id != id || row.version != version {
        return Err(AppError::Conflict("HTTP 配置版本已改变，请重新绑定".into()));
    }
    let _definition = service::mcp_http_definition(config, row)?;
    let fingerprint=canonical::checksum(canonical::json(&json!({"transport":transport,"profile":{"id":row.id.to_hex(),"version":row.version,"kind":row.kind,"configuration":&row.configuration_json,"secret":&row.secret_box,"enabled":row.enabled}}))?.as_bytes());
    Ok(HttpBinding {
        profile_id: id,
        transport: transport.clone(),
        fingerprint,
    })
}
pub(super) async fn resolve(
    db: &Database,
    session: &mut ClientSession,
    config: &Config,
    transport: &Transport,
) -> mongodb::error::Result<Binding> {
    match transport {
        Transport::Stdio { capability_id } => config
            .mcp_programs
            .get(capability_id)
            .cloned()
            .map(Binding::Stdio)
            .ok_or_else(|| fail(Failure::Validation("部署能力未登记"))),
        Transport::Http { .. } => {
            let (id, _) = reference(transport).map_err(error)?;
            let row = db
                .collection::<Profile>("integration_profiles")
                .find_one(doc! {"_id":id,"deletedAt":null})
                .session(session)
                .await?
                .ok_or_else(|| fail(Failure::Validation("HTTP 配置不存在")))?;
            http(config, transport, &row)
                .map(Binding::Http)
                .map_err(error)
        }
    }
}
pub(super) async fn bind(
    db: &Database,
    session: &mut ClientSession,
    config: &Config,
    transport: &Transport,
    actor: &Actor,
) -> mongodb::error::Result<Binding> {
    if matches!(transport, Transport::Http { .. }) {
        if !actor.can("settings.write") {
            return Err(fail(Failure::Forbidden));
        }
        shared::lock(db, "settings", session).await?;
    }
    resolve(db, session, config, transport).await
}
pub(super) async fn current(state: &AppState, row: &Connection) -> bool {
    match &row.transport {
        Transport::Stdio { capability_id } => state
            .config
            .mcp_programs
            .get(capability_id)
            .is_some_and(|v| v.fingerprint() == row.capability_fingerprint),
        Transport::Http { .. } => {
            let Ok((id, _)) = reference(&row.transport) else {
                return false;
            };
            let Ok(db) = state.require_database() else {
                return false;
            };
            let Ok(Some(profile)) = db
                .collection::<Profile>("integration_profiles")
                .find_one(doc! {"_id":id,"deletedAt":null})
                .await
            else {
                return false;
            };
            http(&state.config, &row.transport, &profile)
                .is_ok_and(|v| v.fingerprint == row.capability_fingerprint)
        }
    }
}
impl HttpBinding {
    pub fn allowed_tools(&self) -> &[String] {
        match &self.transport {
            Transport::Http { allowed_tools, .. } => allowed_tools,
            _ => unreachable!(),
        }
    }
    pub async fn spec(
        &self,
        db: &Database,
        config: &Config,
    ) -> Result<tri_mcp_runtime::http::HttpSpec, AppError> {
        let row = db
            .collection::<Profile>("integration_profiles")
            .find_one(doc! {"_id":self.profile_id,"deletedAt":null})
            .await?
            .ok_or(AppError::NotFound)?;
        if http(config, &self.transport, &row)?.fingerprint != self.fingerprint {
            return Err(AppError::Conflict("HTTP 配置绑定已失效".into()));
        }
        let (endpoint, headers) = service::mcp_http_definition(config, &row)?;
        let url = reqwest::Url::parse(&endpoint).map_err(|_| AppError::NotReady)?;
        let client = crate::modules::settings::network::http_client_with_timeout(
            &url,
            config,
            Limits::default().request_timeout,
        )
        .await
        .map_err(|_| AppError::Configuration("HTTP MCP 网络配置不可用".into()))?;
        Ok(tri_mcp_runtime::http::HttpSpec {
            endpoint,
            client,
            headers,
        })
    }
    pub async fn current(&self, db: &Database, config: &Config) -> bool {
        let Ok(Some(row)) = db
            .collection::<Profile>("integration_profiles")
            .find_one(doc! {"_id":self.profile_id,"deletedAt":null})
            .await
        else {
            return false;
        };
        http(config, &self.transport, &row).is_ok_and(|value| value.fingerprint == self.fingerprint)
    }
}
pub(super) async fn ensure(
    db: &Database,
    session: &mut ClientSession,
    config: &Config,
    row: &Connection,
) -> mongodb::error::Result<Binding> {
    let binding = resolve(db, session, config, &row.transport).await?;
    if binding.fingerprint() != row.capability_fingerprint {
        return Err(fail(Failure::Conflict("MCP 配置已改变，请重新确认绑定")));
    }
    Ok(binding)
}
