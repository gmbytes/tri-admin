//! Active package tools are immutable aliases of explicitly implemented business handlers.
use super::{
    models::Extension,
    service,
    tool_binding::{self, CompiledTool},
};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        transaction::{self, Failure, fail},
        validation,
    },
    modules::{
        auth::AuthContext,
        mcp::business,
        rbac::{
            owned,
            shared::{self, Actor},
        },
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{self, Document, doc, oid::ObjectId},
};
use rmcp::{
    ErrorData,
    model::{CallToolRequestParams, CallToolResult, Tool},
};
use serde::Deserialize;
use serde_json::Value;
use tri_extension_package::package::ToolBinding;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolRow {
    #[serde(rename = "_id")]
    id: ObjectId,
    revision_id: ObjectId,
    package: Tools,
}
#[derive(Clone, Deserialize)]
struct Tools {
    tools: Vec<ToolBinding>,
}
fn projection() -> Document {
    doc! {"_id":1,"revisionId":1,"package.tools":1}
}
fn namespace(id: ObjectId, revision: ObjectId, name: &str) -> String {
    format!("plugin.{id}.{revision}.{name}")
}
fn projected_context(context: &AuthContext, actor: &Actor) -> AuthContext {
    let mut current = context.clone();
    current.user.permissions = actor.permissions.clone();
    current.user.data_scope = actor.scope;
    current.user.data_scope_role_ids = actor.scope_role_ids.iter().map(|id| id.to_hex()).collect();
    current.user.role_ids = actor.role_ids.iter().map(|id| id.to_hex()).collect();
    current
}
fn described(row: &ToolRow, context: Option<&AuthContext>) -> Result<Vec<Tool>, AppError> {
    let mut tools = Vec::new();
    for binding in &row.package.tools {
        let compiled = tool_binding::compile(binding)?;
        if context.is_some_and(|context| {
            !context.can(compiled.permission())
                || compiled.operation().starts_with("codegen.") && !context.can("codegen.read")
        }) {
            continue;
        }
        tools.push(compiled.tool(namespace(row.id, row.revision_id, &binding.name)));
    }
    Ok(tools)
}
fn check_budget(tools: &[Tool]) -> Result<(), AppError> {
    let value = serde_json::to_value(tools).map_err(|_| AppError::NotReady)?;
    if tools.len() > 128
        || tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != tools.len()
        || !tool_binding::bounded(&value, 256 * 1024)
    {
        return Err(AppError::Conflict(
            "启用后的 MCP 工具目录超过数量或结构预算".into(),
        ));
    }
    Ok(())
}
pub(super) async fn validate_activation(
    db: &Database,
    session: &mut ClientSession,
    row: &Extension,
) -> mongodb::error::Result<()> {
    let mut cursor=db.collection::<ToolRow>("extensions").find(doc!{"_id":{"$ne":row.id},"enabled":true,"deletedAt":null,"package.tools.0":{"$exists":true}})
        .projection(projection()).limit(129).session(&mut *session).await?;
    let mut rows = cursor.stream(session).try_collect::<Vec<_>>().await?;
    rows.push(ToolRow {
        id: row.id,
        revision_id: row.revision_id,
        package: Tools {
            tools: row.package.tools.clone(),
        },
    });
    tokio::task::spawn_blocking(move || {
        let mut tools = business::contracts()
            .into_iter()
            .map(|contract| contract.tool)
            .collect::<Vec<_>>();
        for row in rows {
            tools.extend(described(&row, None)?);
            check_budget(&tools)?;
        }
        Ok::<_, AppError>(())
    })
    .await
    .map_err(|_| fail(Failure::NotReady))?
    .map_err(|error| match error {
        AppError::Conflict(_) => fail(Failure::Conflict("启用后的 MCP 工具目录超过数量或结构预算")),
        _ => fail(Failure::Validation(
            "插件工具 Schema、目标操作或字段映射无效",
        )),
    })
}
pub(crate) async fn catalog(
    state: &AppState,
    context: &AuthContext,
) -> Result<Vec<Tool>, AppError> {
    let db = state.require_database()?;
    let (actor, rows) = transaction::run_snapshot(
        db,
        (db.clone(), context.clone()),
        |session, (db, context)| {
            async move {
                let actor = owned::fresh(
                    db, session, context,
                    owned::Resource { read: "mcp.read", ..service::policy() }, false,
                ).await?;
                if !actor.can("skills.read") { return Ok((actor, Vec::new())); }
                let mut pipeline = vec![doc! {"$match":{"enabled":true,"deletedAt":null,"package.tools.0":{"$exists":true}}}];
                if actor.scope == DataScope::Own {
                    pipeline.push(doc! {"$match":{"ownerId":actor.id}});
                }
                if actor.scope == DataScope::Roles {
                    pipeline.push(doc! {"$lookup":{"from":"users","localField":"ownerId","foreignField":"_id","pipeline":[{"$match":{"deletedAt":null}},{"$project":{"roleIds":1}}],"as":"__owner"}});
                    pipeline.push(doc! {"$match":{"$or":[{"ownerId":actor.id},{"__owner.roleIds":{"$in":&actor.scope_role_ids}}]}});
                }
                pipeline.push(doc! {"$limit":129});
                pipeline.push(doc! {"$project":projection()});
                let mut cursor = db.collection::<Document>("extensions")
                    .aggregate(pipeline)
                    .max_time(std::time::Duration::from_secs(3))
                    .session(&mut *session).await?;
                let documents = cursor.stream(session).try_collect::<Vec<_>>().await?;
                let rows = documents.into_iter()
                    .map(bson::deserialize_from_document::<ToolRow>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| fail(Failure::NotReady))?;
                Ok((actor, rows))
            }.boxed()
        },
    ).await?;
    let current = projected_context(context, &actor);
    if rows.is_empty() {
        let mut tools = business::catalog(&current);
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        return Ok(tools);
    }
    let permit = state
        .plugin_compile_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut tools = business::catalog(&current);
        for row in rows {
            tools.extend(described(&row, Some(&current))?);
            check_budget(&tools)?;
        }
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tools)
    })
    .await
    .map_err(|_| AppError::Internal)?
}
async fn prepare(
    state: &AppState,
    context: &AuthContext,
    request: &CallToolRequestParams,
) -> Result<(AuthContext, CompiledTool, Value), AppError> {
    context.require("skills.read")?;
    context.require("mcp.invoke")?;
    let mut parts = request.name.splitn(4, '.');
    if parts.next() != Some("plugin") {
        return Err(AppError::NotFound);
    }
    let raw_id = parts.next().ok_or(AppError::NotFound)?;
    let raw_revision = parts.next().ok_or(AppError::NotFound)?;
    let name = parts.next().ok_or(AppError::NotFound)?;
    let id = validation::object_id(raw_id).map_err(|_| AppError::NotFound)?;
    let revision = validation::object_id(raw_revision).map_err(|_| AppError::NotFound)?;
    if id.to_hex() != raw_id || revision.to_hex() != raw_revision {
        return Err(AppError::NotFound);
    }
    let db = state.require_database()?;
    let current = service::get(db, context, id).await?;
    if !current.enabled || current.revision_id != raw_revision {
        return Err(AppError::NotFound);
    }
    let permit = state
        .plugin_compile_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::RateLimited)?;
    let (meta, package) = service::load_package(db, context, id, revision).await?;
    let expected = serde_json::to_value(&meta.package).map_err(|_| AppError::NotReady)?;
    let binding = package
        .tools
        .into_iter()
        .find(|binding| binding.name == name)
        .ok_or(AppError::NotFound)?;
    let arguments = Value::Object(request.arguments.clone().unwrap_or_default());
    let (compiled, arguments) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let compiled = tool_binding::compile(&binding)?;
        let arguments = compiled.arguments(&arguments)?;
        Ok::<_, AppError>((compiled, arguments))
    })
    .await
    .map_err(|_| AppError::Internal)??;
    let permission = compiled.permission();
    let codegen = compiled.operation().starts_with("codegen.");
    let actor = transaction::run(
        db,
        (db.clone(), context.clone(), expected),
        |session, (db, context, expected)| {
            async move {
                let actor = owned::fresh(
                    db,
                    session,
                    context,
                    owned::Resource {
                        write: "mcp.invoke",
                        ..service::policy()
                    },
                    true,
                )
                .await?;
                let row = service::row_tx(db, session, &actor, id, true).await?;
                if !row.enabled || row.revision_id != revision {
                    return Err(fail(Failure::NotFound));
                }
                if serde_json::to_value(&row.package).map_err(|_| fail(Failure::NotReady))?
                    != *expected
                {
                    return Err(fail(Failure::NotReady));
                }
                if !actor.can("mcp.read")
                    || !actor.can(permission)
                    || codegen && !actor.can("codegen.read")
                {
                    return Err(fail(Failure::Forbidden));
                }
                shared::audit(db, &actor, "skills.tool.invoke", id, session).await?;
                Ok(actor)
            }
            .boxed()
        },
    )
    .await?;
    Ok((projected_context(context, &actor), compiled, arguments))
}
pub(crate) async fn call(
    state: &AppState,
    context: &AuthContext,
    mut request: CallToolRequestParams,
) -> Result<CallToolResult, ErrorData> {
    if request.input_responses.is_some() || request.request_state.is_some() {
        return Err(ErrorData::invalid_params("此工具不接受多轮请求续传", None));
    }
    let (current, compiled, arguments) = match prepare(state, context, &request).await {
        Ok(value) => value,
        Err(error) => return Ok(business::error_result(error, false)),
    };
    request.name = compiled.operation().to_owned().into();
    request.arguments = Some(arguments.as_object().cloned().unwrap_or_default());
    business::call(state, &current, request).await
}
