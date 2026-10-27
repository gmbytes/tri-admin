//! Resolves immutable package references to deployment-approved process capabilities.
use super::models::Extension;
use super::service;
use crate::{
    common::{
        canonical,
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{Failure, fail},
        validation,
    },
    config::Config,
    modules::{
        mcp::capabilities::Capability,
        rbac::{owned, shared::Actor},
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{Document, doc, oid::ObjectId},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tri_extension_package::package::{self, Kind, McpContribution, PackageInfo};

pub(crate) struct Reference {
    pub extension_id: ObjectId,
    pub revision_id: ObjectId,
    pub contribution_id: String,
}
impl Reference {
    pub fn parse(extension: &str, revision: &str, contribution: &str) -> Result<Self, AppError> {
        let extension_id = validation::object_id(extension)?;
        let revision_id = validation::object_id(revision)?;
        if extension_id.to_hex() != extension
            || revision_id.to_hex() != revision
            || !package::valid_name(contribution)
        {
            return Err(AppError::Validation("插件 MCP 来源标识无效".into()));
        }
        Ok(Self {
            extension_id,
            revision_id,
            contribution_id: contribution.into(),
        })
    }
}
#[derive(Clone)]
pub(crate) struct PluginProgram {
    pub capability: Capability,
    pub fingerprint: String,
}
fn capability(config: &Config, contribution: &McpContribution) -> Result<Capability, AppError> {
    let mut capability = config
        .mcp_programs
        .get(&contribution.capability_id)
        .cloned()
        .ok_or_else(|| AppError::Validation("插件引用的部署程序未登记".into()))?;
    if contribution
        .tools
        .iter()
        .any(|tool| !capability.allowed_tools.contains(tool))
    {
        return Err(AppError::Validation("插件工具超出部署批准的白名单".into()));
    }
    capability.allowed_tools = contribution.tools.clone();
    Ok(capability)
}
pub(super) fn validate(config: &Config, package: &PackageInfo) -> Result<(), AppError> {
    if !package.mcp.is_empty() && package.kind != Kind::Plugin {
        return Err(AppError::Validation("MCP 来源必须属于插件".into()));
    }
    for contribution in &package.mcp {
        capability(config, contribution)?;
    }
    Ok(())
}
fn resolve(
    config: &Config,
    row: &Extension,
    reference: &Reference,
) -> Result<PluginProgram, AppError> {
    if row.id != reference.extension_id
        || row.kind != Kind::Plugin
        || row.deleted_at.is_some()
        || !row.enabled
        || row.revision_id != reference.revision_id
    {
        return Err(AppError::Conflict("插件已停用、卸载或更换修订".into()));
    }
    let contribution = row
        .package
        .mcp
        .iter()
        .find(|value| value.id == reference.contribution_id)
        .ok_or_else(|| AppError::Validation("插件 MCP 贡献未登记".into()))?;
    let approved = config
        .mcp_programs
        .get(&contribution.capability_id)
        .ok_or_else(|| AppError::Validation("插件引用的部署程序未登记".into()))?;
    let fingerprint=canonical::checksum(canonical::json(&json!({"extensionId":row.id.to_hex(),"revisionId":row.revision_id.to_hex(),"extensionVersion":row.version,"archiveSha256":row.package.archive_sha256,"treeSha256":row.package.tree_sha256,"contribution":contribution,"programFingerprint":approved.fingerprint()}))?.as_bytes());
    Ok(PluginProgram {
        capability: capability(config, contribution)?,
        fingerprint,
    })
}
pub(crate) fn transaction_error(error: AppError) -> mongodb::error::Error {
    match error {
        AppError::Forbidden => fail(Failure::Forbidden),
        AppError::NotFound => fail(Failure::NotFound),
        AppError::Validation(_) => fail(Failure::Validation("插件 MCP 来源或批准工具无效")),
        AppError::Conflict(_) => fail(Failure::Conflict("插件修订或启停版本已改变，请重新绑定")),
        _ => fail(Failure::NotReady),
    }
}
pub(crate) async fn resolve_tx(
    db: &Database,
    session: &mut ClientSession,
    config: &Config,
    reference: &Reference,
    actor: &Actor,
) -> mongodb::error::Result<PluginProgram> {
    if !actor.can("skills.read") {
        return Err(fail(Failure::Forbidden));
    }
    if db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"extensions","version":crate::storage::extensions_schema::VERSION})
        .session(&mut *session)
        .await?
        .is_none()
    {
        return Err(fail(Failure::NotReady));
    }
    let row = db
        .collection::<Extension>("extensions")
        .find_one(doc! {"_id":reference.extension_id,"deletedAt":null})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    owned::owns(db, session, actor, row.owner_id).await?;
    resolve(config, &row, reference).map_err(transaction_error)
}
pub(crate) async fn current(
    db: &Database,
    config: &Config,
    reference: &Reference,
    expected: &str,
) -> bool {
    if !db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"extensions","version":crate::storage::extensions_schema::VERSION})
        .await
        .is_ok_and(|row| row.is_some())
    {
        return false;
    }
    let Ok(Some(row)) = db
        .collection::<Extension>("extensions")
        .find_one(doc! {"_id":reference.extension_id,"deletedAt":null})
        .await
    else {
        return false;
    };
    resolve(config, &row, reference).is_ok_and(|value| value.fingerprint == expected)
}
pub(crate) async fn current_tx(
    db: &Database,
    session: &mut ClientSession,
    config: &Config,
    reference: &Reference,
    expected: &str,
) -> mongodb::error::Result<bool> {
    if db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"extensions","version":crate::storage::extensions_schema::VERSION})
        .session(&mut *session)
        .await?
        .is_none()
    {
        return Ok(false);
    }
    let Some(row) = db
        .collection::<Extension>("extensions")
        .find_one(doc! {"_id":reference.extension_id,"deletedAt":null})
        .session(session)
        .await?
    else {
        return Ok(false);
    };
    Ok(resolve(config, &row, reference).is_ok_and(|value| value.fingerprint == expected))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilityView {
    pub extension_id: String,
    pub extension_name: String,
    pub revision_id: String,
    pub revision: i64,
    pub extension_version: i64,
    pub contribution_id: String,
    pub label: String,
    pub capability_id: String,
    pub allowed_tools: Vec<String>,
    pub current: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryRow {
    #[serde(rename = "_id")]
    id: ObjectId,
    name: String,
    revision_id: ObjectId,
    revision: i64,
    version: i64,
    contribution: McpContribution,
}
pub async fn list(
    state: &AppState,
    context: &crate::modules::auth::AuthContext,
    input: PageInput,
) -> Result<Page<PluginCapabilityView>, AppError> {
    let skip = input.validate()?;
    let mut sort = input.sort_document(&["createdAt", "name"])?;
    sort.insert("package.mcp.id", 1);
    let mut filter =
        doc! {"kind":"plugin","enabled":true,"deletedAt":null,"package.mcp.0":{"$exists":true}};
    if let Some(search) = input
        .search
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        filter.insert("name", doc! {"$regex":service::prefix(search.trim())});
    }
    let db = state.require_database()?;
    let limit = input.page_size;
    let(rows,total)=crate::common::transaction::run_snapshot(db,(db.clone(),context.clone(),filter,sort),|session,(db,context,filter,sort)|{
        async move {
            let actor=owned::fresh(db,session,context,service::policy(),false).await?;
            if !actor.can("mcp.read"){return Err(fail(Failure::Forbidden))}
            let mut filter=filter.clone();if actor.scope==DataScope::Own{filter.insert("ownerId",actor.id);}
            let mut pipeline=vec![doc!{"$match":filter}];
            if actor.scope==DataScope::Roles {
                pipeline.push(doc!{"$lookup":{"from":"users","localField":"ownerId","foreignField":"_id","pipeline":[{"$match":{"deletedAt":null}},{"$project":{"roleIds":1}}],"as":"__owner"}});
                pipeline.push(doc!{"$match":{"$or":[{"ownerId":actor.id},{"__owner.roleIds":{"$in":&actor.scope_role_ids}}]}});
            }
            pipeline.push(doc!{"$unwind":"$package.mcp"});pipeline.push(doc!{"$sort":sort.clone()});
            pipeline.push(doc!{"$facet":{"rows":[{"$skip":skip as i64},{"$limit":limit as i64},{"$project":{"_id":1,"name":1,"revisionId":1,"revision":1,"version":1,"contribution":"$package.mcp"}}],"count":[{"$count":"total"}]}});
            let mut cursor=db.collection::<Document>("extensions").aggregate(pipeline).max_time(std::time::Duration::from_secs(3)).session(&mut *session).await?;
            let result=cursor.stream(session).try_next().await?.ok_or_else(||fail(Failure::NotReady))?;
            let rows=result.get_array("rows").map_err(|_|fail(Failure::NotReady))?.iter().map(|row|mongodb::bson::deserialize_from_bson::<DirectoryRow>(row.clone()).map_err(|_|fail(Failure::NotReady))).collect::<mongodb::error::Result<Vec<_>>>()?;
            let total=result.get_array("count").map_err(|_|fail(Failure::NotReady))?.first().map(|value|value.as_document().and_then(|row|row.get_i64("total").ok().or_else(||row.get_i32("total").ok().map(i64::from))).filter(|value|*value>=0).ok_or_else(||fail(Failure::NotReady))).transpose()?.unwrap_or(0)as u64;
            Ok((rows,total))
        }.boxed()
    }).await?;
    let rows = rows
        .into_iter()
        .map(|row| {
            let current = capability(&state.config, &row.contribution).is_ok();
            PluginCapabilityView {
                extension_id: row.id.to_hex(),
                extension_name: row.name,
                revision_id: row.revision_id.to_hex(),
                revision: row.revision,
                extension_version: row.version,
                contribution_id: row.contribution.id,
                label: row.contribution.label,
                capability_id: row.contribution.capability_id,
                allowed_tools: row.contribution.tools,
                current,
            }
        })
        .collect();
    Ok(Page::new(rows, &input, total))
}
