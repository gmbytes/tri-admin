use super::{
    capabilities::{Capability, CapabilityView},
    dto::{self, ActionInput, ActionKind, Call, Create, Patch},
    models::*,
    runtime,
};
use crate::{
    common::{
        error::AppError,
        model::DataScope,
        pagination::{Page, PageInput},
        transaction::{self, Failure, fail},
        vault,
    },
    modules::{
        auth::AuthContext,
        rbac::{
            owned::{self, Resource},
            shared::{self, Actor},
        },
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    ClientSession, Database,
    bson::{self, DateTime, Document, doc, oid::ObjectId},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
pub(super) fn policy(permission: &'static str) -> Resource {
    Resource {
        schema: "mcp",
        version: crate::storage::mcp_schema::VERSION,
        read: "mcp.read",
        write: permission,
    }
}
pub(super) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn fingerprint<T: Serialize>(
    kind: &str,
    id: Option<ObjectId>,
    input: &T,
) -> Result<String, AppError> {
    Ok(hash(
        &serde_json::to_vec(&(kind, id.map(|v| v.to_hex()), input))
            .map_err(|_| AppError::Internal)?,
    ))
}
fn name(value: &str) -> Result<(), AppError> {
    if value.trim().is_empty() || value.chars().count() > 64 || value.chars().any(char::is_control)
    {
        Err(AppError::Validation("连接名称无效".into()))
    } else {
        Ok(())
    }
}
pub(super) fn cap(state: &AppState, id: &str) -> Result<Capability, AppError> {
    state
        .config
        .mcp_programs
        .get(id)
        .cloned()
        .ok_or_else(|| AppError::Validation("部署能力未登记".into()))
}
fn current_cap(state: &AppState, row: &Connection) -> bool {
    state
        .config
        .mcp_programs
        .get(row.transport.capability())
        .is_some_and(|cap| cap.fingerprint() == row.capability_fingerprint)
}
fn view(state: &AppState, mut row: Connection) -> Result<ConnectionView, AppError> {
    let current = current_cap(state, &row);
    if let Some(runtime) = row.runtime.as_mut()
        && !runtime.exit_confirmed
        && runtime.state != "retired"
        && runtime.service_id != state.mcp.service_id
    {
        runtime.state = "unconfirmed".into();
    }
    row.view(current)
}
pub(super) async fn connection_tx(
    db: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    id: ObjectId,
    active: bool,
) -> mongodb::error::Result<Connection> {
    let mut filter = doc! {"_id":id};
    if active {
        filter.insert("deletedAt", bson::Bson::Null);
    }
    let row = db
        .collection::<Connection>("mcp_connections")
        .find_one(filter)
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    owned::owns(db, session, actor, row.owner_id).await?;
    Ok(row)
}
pub(super) async fn action_tx(
    db: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    id: ObjectId,
) -> mongodb::error::Result<Action> {
    let row = db
        .collection::<Action>("mcp_actions")
        .find_one(doc! {"_id":id})
        .session(&mut *session)
        .await?
        .ok_or_else(|| fail(Failure::NotFound))?;
    owned::owns(db, session, actor, row.owner_id).await?;
    Ok(row)
}
pub(super) async fn replay_tx(
    db: &Database,
    session: &mut ClientSession,
    actor: &Actor,
    request: &str,
    fingerprint: &str,
) -> mongodb::error::Result<Option<Action>> {
    let row = db
        .collection::<Action>("mcp_actions")
        .find_one(doc! {"actorId":actor.id,"requestId":request})
        .session(&mut *session)
        .await?;
    if let Some(row) = row {
        owned::owns(db, session, actor, row.owner_id).await?;
        if row.fingerprint != fingerprint {
            return Err(fail(Failure::Conflict("请求标识已用于其他 MCP 操作")));
        }
        Ok(Some(row))
    } else {
        Ok(None)
    }
}
fn action_view(mut row: Action, actor: &Actor) -> Result<ActionView, AppError> {
    if row.actor_id != actor.id || !actor.can("mcp.invoke") {
        row.arguments_sha256 = None;
        row.result_sha256 = None;
    }
    row.view()
}
pub async fn capabilities(
    state: &AppState,
    context: &AuthContext,
) -> Result<Vec<CapabilityView>, AppError> {
    let db = state.require_database()?;
    transaction::run_snapshot(
        db,
        (db.clone(), context.clone()),
        |session, (db, context)| {
            async move {
                owned::fresh(db, session, context, policy("mcp.write"), false).await?;
                Ok(())
            }
            .boxed()
        },
    )
    .await?;
    Ok(state.config.mcp_programs.views())
}
pub async fn create(
    state: &AppState,
    context: &AuthContext,
    input: Create,
) -> Result<(ConnectionView, bool), AppError> {
    dto::request_id(&input.request_id)?;
    name(&input.name)?;
    let catalog = state.config.mcp_programs.clone();
    let fp = fingerprint("connection.create", None, &input)?;
    let db = state.require_database()?;
    let id = ObjectId::new();
    let now = DateTime::now();
    let (row, created) = transaction::run(
        db,
        (db.clone(), context.clone(), input, fp, catalog),
        |session, (db, context, input, fp, catalog)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), true).await?;
                if let Some(old) = db
                    .collection::<Connection>("mcp_connections")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&input.request_id})
                    .session(&mut *session)
                    .await?
                {
                    if old.fingerprint != *fp {
                        return Err(fail(Failure::Conflict("请求标识已用于不同连接")));
                    }
                    return Ok((old, false));
                }
                let capability = catalog
                    .get(input.transport.capability())
                    .ok_or_else(|| fail(Failure::Validation("部署能力未登记")))?;
                let row = Connection {
                    id,
                    owner_id: actor.id,
                    request_id: input.request_id.clone(),
                    fingerprint: fp.clone(),
                    name: input.name.trim().into(),
                    transport: input.transport.clone(),
                    capability_fingerprint: capability.fingerprint(),
                    enabled: input.enabled,
                    version: 1,
                    runtime: None,
                    control_action_id: None,
                    created_at: now,
                    updated_at: now,
                    deleted_at: None,
                };
                db.collection::<Connection>("mcp_connections")
                    .insert_one(&row)
                    .session(&mut *session)
                    .await?;
                shared::audit(db, &actor, "mcp.connection.create", id, session).await?;
                Ok((row, true))
            }
            .boxed()
        },
    )
    .await?;
    Ok((view(state, row)?, created))
}
pub async fn get(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
) -> Result<ConnectionView, AppError> {
    let db = state.require_database()?;
    let row = transaction::run_snapshot(
        db,
        (db.clone(), context.clone()),
        |session, (db, context)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), false).await?;
                connection_tx(db, session, &actor, id, true).await
            }
            .boxed()
        },
    )
    .await?;
    view(state, row)
}
pub async fn by_request(
    state: &AppState,
    context: &AuthContext,
    request: &str,
) -> Result<ConnectionView, AppError> {
    dto::request_id(request)?;
    let db = state.require_database()?;
    let row = transaction::run_snapshot(
        db,
        (db.clone(), context.clone(), request.to_owned()),
        |session, (db, context, request)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), false).await?;
                db.collection::<Connection>("mcp_connections")
                    .find_one(doc! {"ownerId":actor.id,"requestId":&*request})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))
            }
            .boxed()
        },
    )
    .await?;
    view(state, row)
}
pub async fn patch(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
    input: Patch,
) -> Result<ConnectionView, AppError> {
    dto::version(input.version)?;
    name(&input.name)?;
    let capability = cap(state, input.transport.capability())?;
    let db = state.require_database()?;
    let row = transaction::run(
        db,
        (db.clone(), context.clone(), input, capability),
        |session, (db, context, input, capability)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), true).await?;
                let mut row = connection_tx(db, session, &actor, id, true).await?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("连接版本已改变")));
                }
                if row.control_action_id.is_some()
                    || row
                        .runtime
                        .as_ref()
                        .is_some_and(|r| !r.exit_confirmed && r.state != "retired")
                {
                    return Err(fail(Failure::Conflict("请先确认当前实例退出")));
                }
                row.name = input.name.trim().into();
                row.transport = input.transport.clone();
                row.capability_fingerprint = capability.fingerprint();
                row.enabled = input.enabled;
                row.version += 1;
                row.updated_at = DateTime::now();
                db.collection::<Connection>("mcp_connections")
                    .replace_one(doc! {"_id":id,"version":input.version}, &row)
                    .session(&mut *session)
                    .await?;
                shared::audit(db, &actor, "mcp.connection.update", id, session).await?;
                Ok(row)
            }
            .boxed()
        },
    )
    .await?;
    view(state, row)
}
pub async fn delete(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
    version: i64,
) -> Result<(), AppError> {
    dto::version(version)?;
    let db = state.require_database()?;
    transaction::run(db,(db.clone(),context.clone()),|session,(db,context)|async move{let actor=owned::fresh(db,session,context,policy("mcp.write"),true).await?;let row=connection_tx(db,session,&actor,id,true).await?;if row.version!=version{return Err(fail(Failure::Conflict("连接版本已改变")))}
if row.control_action_id.is_some()||row.runtime.as_ref().is_some_and(|r|!r.exit_confirmed&&r.state!="retired"){return Err(fail(Failure::Conflict("请先确认当前实例退出")))}let now=DateTime::now();db.collection::<Document>("mcp_connections").update_one(doc!{"_id":id,"version":version},doc!{"$set":{"deletedAt":now,"updatedAt":now,"enabled":false},"$inc":{"version":1i64}}).session(&mut *session).await?;shared::audit(db,&actor,"mcp.connection.delete",id,session).await}.boxed()).await
}
fn escaped_prefix(value: &str) -> String {
    let mut out = String::from("^");
    for c in value.chars() {
        if ".+*?^${}()|[]\\".contains(c) {
            out.push('\\')
        }
        out.push(c)
    }
    out
}
async fn page<T: DeserializeOwned + Send>(
    state: &AppState,
    context: &AuthContext,
    collection: &'static str,
    input: &PageInput,
    filter: Document,
) -> Result<(Vec<T>, u64, Actor), AppError> {
    let skip = input.validate()?;
    let sort = input.sort_document(&["createdAt"])?;
    let size = input.page_size;
    let db = state.require_database()?;
    transaction::run_snapshot(db,(db.clone(),context.clone(),filter,sort),|session,(db,context,filter,sort)|async move{let actor=owned::fresh(db,session,context,policy("mcp.write"),false).await?;let mut filter=filter.clone();if actor.scope==DataScope::Own{filter.insert("ownerId",actor.id);}let mut pipeline=vec![doc!{"$match":filter}];if actor.scope==DataScope::Roles{pipeline.push(doc!{"$lookup":{"from":"users","localField":"ownerId","foreignField":"_id","pipeline":[{"$match":{"deletedAt":null}},{"$project":{"roleIds":1}}],"as":"__scopeOwner"}});pipeline.push(doc!{"$match":{"$or":[{"ownerId":actor.id},{"__scopeOwner.roleIds":{"$in":&actor.scope_role_ids}}]}});}pipeline.push(doc!{"$sort":sort.clone()});pipeline.push(doc!{"$facet":{"rows":[{"$skip":skip as i64},{"$limit":size as i64},{"$project":{"__scopeOwner":0}}],"count":[{"$count":"total"}]}});let mut cursor=db.collection::<Document>(collection).aggregate(pipeline).max_time(std::time::Duration::from_secs(3)).session(&mut *session).await?;let row=cursor.stream(session).try_next().await?.ok_or_else(||fail(Failure::NotReady))?;let rows=row.get_array("rows").map_err(|_|fail(Failure::NotReady))?.iter().map(|v|bson::deserialize_from_bson(v.clone()).map_err(|_|fail(Failure::NotReady))).collect::<mongodb::error::Result<Vec<T>>>()?;let counts=row.get_array("count").map_err(|_|fail(Failure::NotReady))?;let total=if let Some(row)=counts.first(){let count=row.as_document().and_then(|v|v.get("total")).and_then(|n|n.as_i64().or_else(||n.as_i32().map(i64::from))).filter(|v|*v>=0).ok_or_else(||fail(Failure::NotReady))?;count as u64}else{0};Ok((rows,total,actor))}.boxed()).await
}
pub async fn connections(
    state: &AppState,
    context: &AuthContext,
    input: PageInput,
) -> Result<Page<ConnectionView>, AppError> {
    let mut filter = doc! {"deletedAt":null};
    if let Some(search) = input.search.as_deref().filter(|v| !v.trim().is_empty()) {
        filter.insert("name", doc! {"$regex":escaped_prefix(search.trim())});
    }
    let (rows, total, _) =
        page::<Connection>(state, context, "mcp_connections", &input, filter).await?;
    Ok(Page::new(
        rows.into_iter()
            .map(|row| view(state, row))
            .collect::<Result<_, _>>()?,
        &input,
        total,
    ))
}
pub async fn actions(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
    input: PageInput,
) -> Result<Page<ActionView>, AppError> {
    get(state, context, id).await?;
    let (rows, total, actor) = page::<Action>(
        state,
        context,
        "mcp_actions",
        &input,
        doc! {"connectionId":id},
    )
    .await?;
    Ok(Page::new(
        rows.into_iter()
            .map(|row| action_view(row, &actor))
            .collect::<Result<_, _>>()?,
        &input,
        total,
    ))
}
pub async fn get_action(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
) -> Result<ActionView, AppError> {
    let db = state.require_database()?;
    let (row, actor) = transaction::run_snapshot(
        db,
        (db.clone(), context.clone()),
        |session, (db, context)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), false).await?;
                let row = action_tx(db, session, &actor, id).await?;
                Ok((row, actor))
            }
            .boxed()
        },
    )
    .await?;
    action_view(row, &actor)
}
pub async fn action_by_request(
    state: &AppState,
    context: &AuthContext,
    request: &str,
) -> Result<ActionView, AppError> {
    dto::request_id(request)?;
    let db = state.require_database()?;
    let (row, actor) = transaction::run_snapshot(
        db,
        (db.clone(), context.clone(), request.to_owned()),
        |session, (db, context, request)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), false).await?;
                let row = db
                    .collection::<Action>("mcp_actions")
                    .find_one(doc! {"actorId":actor.id,"requestId":&*request})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                owned::owns(db, session, &actor, row.owner_id).await?;
                Ok((row, actor))
            }
            .boxed()
        },
    )
    .await?;
    action_view(row, &actor)
}
pub(super) fn new_action(
    actor: &Actor,
    row: &Connection,
    id: ObjectId,
    request: &str,
    fp: &str,
    kind: &str,
    instance: Option<ObjectId>,
) -> Action {
    Action {
        id,
        actor_id: actor.id,
        owner_id: row.owner_id,
        connection_id: row.id,
        request_id: request.into(),
        fingerprint: fp.into(),
        kind: kind.into(),
        status: "pending".into(),
        connection_version: row.version,
        instance_id: instance,
        target_action_id: None,
        name: None,
        arguments_sha256: None,
        error_code: None,
        result_kind: None,
        result_size: None,
        result_sha256: None,
        created_at: DateTime::now(),
        finished_at: None,
        cancel_requested: false,
    }
}
pub async fn submit_action(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
    input: ActionInput,
) -> Result<(ActionView, bool), AppError> {
    dto::request_id(&input.request_id)?;
    dto::version(input.version)?;
    let fp = fingerprint(input.kind.code(), Some(id), &input)?;
    let db = state.require_database()?;
    let action_id = ObjectId::new();
    let instance_id = ObjectId::new();
    let service_id = state.mcp.service_id.clone();
    let (result, created, old_instance, capability) = transaction::run(
        db,
        (db.clone(), context.clone(), input, fp, state.clone()),
        |session, (db, context, input, fp, state)| {
            let service_id = service_id.clone();
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), true).await?;
                if let Some(row) = replay_tx(db, session, &actor, &input.request_id, fp).await? {
                    return Ok((row, false, None, None));
                }
                let mut row = connection_tx(db, session, &actor, id, true).await?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("连接版本已改变")));
                }
                if row.control_action_id.is_some() && !matches!(input.kind, ActionKind::Stop) {
                    return Err(fail(Failure::Conflict("已有生命周期操作待确认")));
                }
                if matches!(input.kind, ActionKind::Stop)
                    && let Some(previous) = row.control_action_id
                {
                    db.collection::<Document>("mcp_actions")
                        .update_one(
                            doc! {"_id":previous,"status":{"$in":["pending","running"]}},
                            doc! {"$set":{"cancelRequested":true}},
                        )
                        .session(&mut *session)
                        .await?;
                }
                let old = row.runtime.clone();
                let starts = matches!(input.kind, ActionKind::Start | ActionKind::Restart);
                let capability = if starts {
                    if !row.enabled {
                        return Err(fail(Failure::Conflict("连接已停用")));
                    }
                    let capability = state
                        .config
                        .mcp_programs
                        .get(row.transport.capability())
                        .cloned()
                        .ok_or_else(|| fail(Failure::Validation("部署能力未登记")))?;
                    if capability.fingerprint() != row.capability_fingerprint {
                        return Err(fail(Failure::Conflict("部署能力已改变，请重新确认绑定")));
                    }
                    Some(capability)
                } else {
                    None
                };
                if matches!(input.kind, ActionKind::Start)
                    && old
                        .as_ref()
                        .is_some_and(|v| !v.exit_confirmed && v.state != "retired")
                {
                    return Err(fail(Failure::Conflict("原实例退出尚未确认")));
                }
                if matches!(input.kind, ActionKind::Stop | ActionKind::Restart) {
                    let old = old
                        .as_ref()
                        .ok_or_else(|| fail(Failure::Conflict("没有可停止的实例")))?;
                    if old.exit_confirmed || old.state == "retired" {
                        return Err(fail(Failure::Conflict("实例已经结束")));
                    }
                    if old.service_id != service_id
                        || old.lease_until < DateTime::now()
                        || state.mcp.get(old.id).is_none()
                    {
                        return Err(fail(Failure::Conflict(
                            "原服务实例不可控，需要确认遗留进程",
                        )));
                    }
                }
                if starts {
                    let filter = doc! {"exitConfirmed":false,"state":{"$ne":"retired"}};
                    let count = db
                        .collection::<Document>("mcp_instances")
                        .count_documents(filter.clone())
                        .session(&mut *session)
                        .await?;
                    let own = db
                        .collection::<Document>("mcp_instances")
                        .count_documents(doc! {"$and":[filter,{"ownerId":row.owner_id}]})
                        .session(&mut *session)
                        .await?;
                    let extra = if matches!(input.kind, ActionKind::Restart) {
                        1
                    } else {
                        0
                    };
                    if count >= 16 + extra || own >= 4 + extra {
                        return Err(fail(Failure::RateLimited));
                    }
                }
                row.version += 1;
                row.updated_at = DateTime::now();
                row.control_action_id = Some(action_id);
                if matches!(input.kind, ActionKind::Stop | ActionKind::Restart)
                    && let Some(runtime) = row.runtime.as_mut()
                {
                    runtime.state = "stopping".into();
                    db.collection::<Document>("mcp_instances")
                        .update_one(
                            doc! {"_id":runtime.id,"exitConfirmed":false},
                            doc! {"$set":{"state":"stopping","updatedAt":DateTime::now()}},
                        )
                        .session(&mut *session)
                        .await?;
                }
                let target = if starts {
                    Some(instance_id)
                } else {
                    old.as_ref().map(|v| v.id)
                };
                let mut action = new_action(
                    &actor,
                    &row,
                    action_id,
                    &input.request_id,
                    fp,
                    input.kind.code(),
                    target,
                );
                action.connection_version = row.version;
                if starts {
                    let now = DateTime::now();
                    let instance = Instance {
                        id: instance_id,
                        connection_id: id,
                        owner_id: row.owner_id,
                        actor_id: actor.id,
                        service_id: service_id.clone(),
                        state: if matches!(input.kind, ActionKind::Restart) {
                            "reserved"
                        } else {
                            "starting"
                        }
                        .into(),
                        pid: None,
                        exit_confirmed: false,
                        lease_until: DateTime::from_millis(now.timestamp_millis() + 15000),
                        tools_json: "[]".into(),
                        error_code: None,
                        created_at: now,
                        updated_at: now,
                    };
                    db.collection::<Instance>("mcp_instances")
                        .insert_one(&instance)
                        .session(&mut *session)
                        .await?;
                    if matches!(input.kind, ActionKind::Start) {
                        row.runtime = Some(RuntimeMeta {
                            id: instance_id,
                            service_id: service_id.clone(),
                            state: "starting".into(),
                            pid: None,
                            exit_confirmed: false,
                            lease_until: instance.lease_until,
                            updated_at: now,
                            error_code: None,
                        });
                    }
                }
                db.collection::<Connection>("mcp_connections")
                    .replace_one(doc! {"_id":id,"version":input.version}, &row)
                    .session(&mut *session)
                    .await?;
                db.collection::<Action>("mcp_actions")
                    .insert_one(&action)
                    .session(&mut *session)
                    .await?;
                shared::audit(
                    db,
                    &actor,
                    &format!("mcp.{}", input.kind.code()),
                    id,
                    session,
                )
                .await?;
                Ok((action, true, old.map(|v| v.id), capability))
            }
            .boxed()
        },
    )
    .await?;
    if created {
        runtime::launch_action(
            state,
            context.clone(),
            result.clone(),
            old_instance,
            capability,
        );
    }
    Ok((result.view()?, created))
}
pub async fn tools(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
) -> Result<Vec<Value>, AppError> {
    let db = state.require_database()?;
    let json = transaction::run_snapshot(
        db,
        (db.clone(), context.clone()),
        |session, (db, context)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), false).await?;
                let row = connection_tx(db, session, &actor, id, true).await?;
                let runtime = row
                    .runtime
                    .ok_or_else(|| fail(Failure::Conflict("连接尚未运行")))?;
                let instance = db
                    .collection::<Instance>("mcp_instances")
                    .find_one(doc! {"_id":runtime.id})
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                Ok(instance.tools_json)
            }
            .boxed()
        },
    )
    .await?;
    if json.len() > 256 * 1024 {
        return Err(AppError::NotReady);
    }
    serde_json::from_str(&json).map_err(|_| AppError::NotReady)
}
pub async fn submit_call(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
    input: Call,
) -> Result<(ActionView, bool), AppError> {
    dto::request_id(&input.request_id)?;
    dto::version(input.version)?;
    if input.name.trim().is_empty()
        || input.name.chars().count() > 128
        || input.name.chars().any(char::is_control)
        || !input.arguments.is_object()
        || serde_json::to_vec(&input.arguments)
            .map_err(|_| AppError::BadRequest)?
            .len()
            > 64 * 1024
    {
        return Err(AppError::Validation("工具名或参数无效".into()));
    }
    let fp = fingerprint("call", Some(id), &input)?;
    let db = state.require_database()?;
    let action_id = ObjectId::new();
    let (action, created) = transaction::run(
        db,
        (
            db.clone(),
            context.clone(),
            input.clone(),
            fp,
            state.clone(),
        ),
        |session, (db, context, input, fp, state)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.invoke"), true).await?;
                if let Some(row) = replay_tx(db, session, &actor, &input.request_id, fp).await? {
                    return Ok((row, false));
                }
                if state.config.settings_active_key().is_none() {
                    return Err(fail(Failure::SecretStoreUnavailable));
                }
                let row = connection_tx(db, session, &actor, id, true).await?;
                if row.version != input.version {
                    return Err(fail(Failure::Conflict("连接版本已改变")));
                }
                if !row.enabled || row.control_action_id.is_some() {
                    return Err(fail(Failure::Conflict("连接不接受新调用")));
                }
                let runtime = row
                    .runtime
                    .as_ref()
                    .ok_or_else(|| fail(Failure::Conflict("连接尚未运行")))?;
                if runtime.state != "running"
                    || runtime.exit_confirmed
                    || runtime.lease_until < DateTime::now()
                    || runtime.service_id != state.mcp.service_id
                {
                    return Err(fail(Failure::Conflict("当前服务没有可用实例")));
                }
                let capability = state
                    .config
                    .mcp_programs
                    .get(row.transport.capability())
                    .ok_or_else(|| fail(Failure::Validation("部署能力未登记")))?;
                if capability.fingerprint() != row.capability_fingerprint
                    || !capability.allowed_tools.contains(&input.name)
                {
                    return Err(fail(Failure::Validation("工具未获部署能力批准")));
                }
                let instance = db
                    .collection::<Instance>("mcp_instances")
                    .find_one(doc! {"_id":runtime.id})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotReady))?;
                let tools: Vec<Value> = serde_json::from_str(&instance.tools_json)
                    .map_err(|_| fail(Failure::NotReady))?;
                if !tools
                    .iter()
                    .any(|tool| tool["name"].as_str() == Some(&input.name))
                {
                    return Err(fail(Failure::Validation("工具不在当前发现清单")));
                }
                let count = db
                    .collection::<Document>("mcp_actions")
                    .count_documents(doc! {"status":{"$in":["pending","running"]}})
                    .session(&mut *session)
                    .await?;
                if count >= 64 {
                    return Err(fail(Failure::RateLimited));
                }
                let mut action = new_action(
                    &actor,
                    &row,
                    action_id,
                    &input.request_id,
                    fp,
                    "call",
                    Some(runtime.id),
                );
                action.name = Some(input.name.clone());
                action.arguments_sha256 = Some(hash(
                    &serde_json::to_vec(&input.arguments).map_err(|_| fail(Failure::NotReady))?,
                ));
                db.collection::<Action>("mcp_actions")
                    .insert_one(&action)
                    .session(&mut *session)
                    .await?;
                shared::audit(db, &actor, "mcp.call", id, session).await?;
                Ok((action, true))
            }
            .boxed()
        },
    )
    .await?;
    if created {
        runtime::launch_call(state, context.clone(), action.clone(), input.arguments);
    }
    Ok((action.view()?, created))
}
pub(super) fn result_aad(action: &Action) -> Vec<u8> {
    format!(
        "tri-admin/mcp/result/v1/{}/{}/{}",
        action.id, action.actor_id, action.connection_id
    )
    .into_bytes()
}
pub async fn result(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
) -> Result<ResultView, AppError> {
    let db = state.require_database()?;
    let (action, record) = transaction::run_snapshot(
        db,
        (db.clone(), context.clone()),
        |session, (db, context)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.invoke"), false).await?;
                if !actor.can("mcp.invoke") {
                    return Err(fail(Failure::Forbidden));
                }
                let action = action_tx(db, session, &actor, id).await?;
                if action.actor_id != actor.id {
                    return Err(fail(Failure::Forbidden));
                }
                let record = db
                    .collection::<ResultRecord>("mcp_results")
                    .find_one(
                        doc! {"_id":id,"actorId":actor.id,"expiresAt":{"$gt":DateTime::now()}},
                    )
                    .session(session)
                    .await?
                    .ok_or_else(|| fail(Failure::NotFound))?;
                if record.owner_id != action.owner_id
                    || record.connection_id != action.connection_id
                {
                    return Err(fail(Failure::NotReady));
                }
                Ok((action, record))
            }
            .boxed()
        },
    )
    .await?;
    let bytes = vault::open_bounded(
        &state.config,
        &result_aad(&action),
        &record.sealed,
        512 * 1024,
    )?;
    let sha = hash(&bytes);
    let size = bytes.len() as i64;
    if action.result_size != Some(size) || action.result_sha256.as_deref() != Some(&sha) {
        return Err(AppError::NotReady);
    }
    Ok(ResultView {
        content: String::from_utf8(bytes.to_vec()).map_err(|_| AppError::NotReady)?,
        size,
        sha256: sha,
        kind: action.result_kind.ok_or(AppError::NotReady)?,
    })
}

pub async fn cancel_call(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
    input: dto::CancelInput,
) -> Result<(ActionView, bool), AppError> {
    dto::request_id(&input.request_id)?;
    let fp = fingerprint("call.cancel", Some(id), &input)?;
    let action_id = ObjectId::new();
    let db = state.require_database()?;
    let (row, created) = transaction::run(
        db,
        (db.clone(), context.clone(), input, fp),
        |session, (db, context, input, fp)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.invoke"), true).await?;
                if let Some(old) = replay_tx(db, session, &actor, &input.request_id, fp).await? {
                    return Ok((old, false));
                }
                let target = action_tx(db, session, &actor, id).await?;
                if target.kind != "call"
                    || target.actor_id != actor.id
                        && (!actor.can("mcp.write") || actor.scope != DataScope::All)
                {
                    return Err(fail(Failure::Forbidden));
                }
                if !["pending", "running"].contains(&target.status.as_str()) {
                    return Err(fail(Failure::Conflict("原调用已经结束，请读取原回执")));
                }
                let connection =
                    connection_tx(db, session, &actor, target.connection_id, false).await?;
                let mut action = new_action(
                    &actor,
                    &connection,
                    action_id,
                    &input.request_id,
                    fp,
                    "cancel",
                    target.instance_id,
                );
                action.target_action_id = Some(id);
                action.status = "succeeded".into();
                action.finished_at = Some(DateTime::now());
                db.collection::<Document>("mcp_actions")
                    .update_one(
                        doc! {"_id":id,"status":{"$in":["pending","running"]}},
                        doc! {"$set":{"cancelRequested":true}},
                    )
                    .session(&mut *session)
                    .await?;
                db.collection::<Action>("mcp_actions")
                    .insert_one(&action)
                    .session(&mut *session)
                    .await?;
                shared::audit(db, &actor, "mcp.call.cancel", id, session).await?;
                Ok((action, true))
            }
            .boxed()
        },
    )
    .await?;
    if created && let Some(entry) = row.instance_id.and_then(|id| state.mcp.get(id)) {
        entry.cancel_call(id);
    }
    Ok((row.view()?, created))
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceReport {
    pub scanned: u64,
    pub orphaned: u64,
    pub stopping: u64,
}
pub async fn maintenance_once(state: &AppState) -> Result<MaintenanceReport, AppError> {
    let db = state.require_database()?;
    if !crate::storage::schema::is_initialized(db).await? {
        return Err(AppError::NotReady);
    }
    let now = DateTime::now();
    let rows=db.collection::<Instance>("mcp_instances").find(doc!{"exitConfirmed":false,"state":{"$in":["reserved","starting","running","stopping"]},"leaseUntil":{"$lte":now}}).sort(doc!{"leaseUntil":1,"_id":1}).limit(64).await?.try_collect::<Vec<_>>().await?;
    let mut report = MaintenanceReport {
        scanned: rows.len() as u64,
        ..Default::default()
    };
    for row in rows {
        if let Some(entry) = state.mcp.get(row.id) {
            entry.stop.cancel();
            report.stopping += 1;
            continue;
        }
        let changed=transaction::run(db,(db.clone(),row),|session,(db,row)|async move{shared::lock(db,"mcp",session).await?;let changed=db.collection::<Document>("mcp_instances").update_one(doc!{"_id":row.id,"exitConfirmed":false,"state":{"$in":["reserved","starting","running","stopping"]},"leaseUntil":{"$lte":DateTime::now()}},doc!{"$set":{"state":"orphaned","errorCode":"SERVICE_LEASE_EXPIRED","updatedAt":DateTime::now()}}).session(&mut *session).await?;if changed.modified_count==0{return Ok(false)}db.collection::<Document>("mcp_connections").update_one(doc!{"_id":row.connection_id,"runtime.id":row.id},doc!{"$set":{"runtime.state":"orphaned","runtime.errorCode":"SERVICE_LEASE_EXPIRED","controlActionId":null}}).session(&mut *session).await?;db.collection::<Document>("mcp_actions").update_many(doc!{"instanceId":row.id,"status":{"$in":["pending","running"]}},doc!{"$set":{"status":"unknown","errorCode":"SERVICE_LEASE_EXPIRED","finishedAt":DateTime::now()}}).session(&mut *session).await?;db.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"administration","action":"mcp.orphaned","actorId":row.actor_id,"targetId":row.id,"outcome":"success","observer":true,"createdAt":DateTime::now()})).session(session).await?;Ok(true)}.boxed()).await?;
        if changed {
            report.orphaned += 1;
        }
    }
    Ok(report)
}
pub async fn confirm_orphan(
    state: &AppState,
    context: &AuthContext,
    id: ObjectId,
    input: dto::OrphanConfirm,
) -> Result<ActionView, AppError> {
    context.require_session()?;
    dto::request_id(&input.request_id)?;
    dto::version(input.version)?;
    if !input.confirmed
        || input.reason.trim().is_empty()
        || input.reason.chars().count() > 200
        || input.reason.chars().any(char::is_control)
    {
        return Err(AppError::Validation("需要明确确认与说明".into()));
    }
    let fp = fingerprint("orphan.confirm", Some(id), &input)?;
    let action_id = ObjectId::new();
    let db = state.require_database()?;
    let result=transaction::run(db,(db.clone(),context.clone(),input,fp,state.clone()),|session,(db,context,input,fp,state)|async move{let actor=owned::fresh(db,session,context,policy("mcp.write"),true).await?;if actor.scope!=DataScope::All{return Err(fail(Failure::Forbidden))}
if let Some(old)=replay_tx(db,session,&actor,&input.request_id,fp).await?{return Ok(old)}let mut row=connection_tx(db,session,&actor,id,true).await?;if row.version!=input.version{return Err(fail(Failure::Conflict("连接版本已改变")))}let old=row.runtime.as_ref().ok_or_else(||fail(Failure::Conflict("没有遗留实例")))?;if old.exit_confirmed||old.state=="retired"||old.lease_until>DateTime::now()||state.mcp.get(old.id).is_some(){return Err(fail(Failure::Conflict("实例仍有可验证的所有权或已经结束")))}let instance_id=old.id;let mut action=new_action(&actor,&row,action_id,&input.request_id,fp,"orphan.confirm",Some(instance_id));action.status="succeeded".into();action.error_code=Some("OPERATOR_CONFIRMED".into());action.finished_at=Some(DateTime::now());row.version+=1;row.control_action_id=None;row.updated_at=DateTime::now();let runtime=row.runtime.as_mut().unwrap();runtime.state="retired".into();runtime.pid=None;runtime.error_code=Some("OPERATOR_CONFIRMED".into());runtime.updated_at=DateTime::now();db.collection::<Document>("mcp_instances").update_one(doc!{"_id":instance_id,"exitConfirmed":false},doc!{"$set":{"state":"retired","errorCode":"OPERATOR_CONFIRMED","updatedAt":DateTime::now()}}).session(&mut *session).await?;db.collection::<Connection>("mcp_connections").replace_one(doc!{"_id":id,"version":input.version},&row).session(&mut *session).await?;db.collection::<Document>("mcp_actions").update_many(doc!{"instanceId":instance_id,"status":{"$in":["pending","running"]}},doc!{"$set":{"status":"unknown","errorCode":"OPERATOR_CONFIRMED","finishedAt":DateTime::now()}}).session(&mut *session).await?;db.collection::<Action>("mcp_actions").insert_one(&action).session(&mut *session).await?;db.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"administration","action":"mcp.orphan.confirm","actorId":actor.id,"targetId":instance_id,"outcome":"success","reason":&input.reason,"createdAt":DateTime::now()})).session(session).await?;Ok(action)}.boxed()).await?;
    result.view()
}
pub fn start_service(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {_ = state.mcp.shutdown.cancelled()=>break,_ = timer.tick()=>{let _=maintenance_once(&state).await;}}
        }
    })
}
pub fn stop_service(state: &AppState) {
    state.mcp.stop_all();
}
pub async fn shutdown(state: &AppState) {
    state.mcp.wait_shutdown().await;
}
