use super::{
    binding::{self, Binding, HttpBinding},
    capabilities::Capability,
    models::*,
    registry::Entry,
    service::{self, connection_tx, policy},
};
use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
        vault,
    },
    config::Config,
    modules::{
        auth::AuthContext,
        extensions::programs::PluginProgram,
        rbac::{owned, shared},
    },
    state::AppState,
};
use futures_util::FutureExt;
use mongodb::{
    ClientSession, Database,
    bson::{self, DateTime, Document, doc, oid::ObjectId},
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use tri_mcp_runtime::{
    process::{Phase, Process},
    session::{Error as RuntimeError, Reply, ReplyKind},
};
fn lease() -> DateTime {
    DateTime::from_millis(DateTime::now().timestamp_millis() + 15000)
}
fn code(error: &RuntimeError) -> &'static str {
    match error {
        RuntimeError::InvalidInput => "INVALID_INPUT",
        RuntimeError::Spawn => "SPAWN_FAILED",
        RuntimeError::StartRejected => "DISCOVERY_FAILED",
        RuntimeError::StartTimeout => "DISCOVERY_TIMEOUT",
        RuntimeError::Limit => "RESPONSE_LIMIT",
        RuntimeError::CancelledBeforeSend => "CANCELLED_BEFORE_SEND",
        RuntimeError::BusyBeforeSend => "BUSY_BEFORE_SEND",
        RuntimeError::CancelledUnknown => "CANCELLED_UNKNOWN",
        RuntimeError::TimeoutUnknown => "TIMEOUT_UNKNOWN",
        RuntimeError::TransportUnknown => "TRANSPORT_UNKNOWN",
        RuntimeError::RemoteRejected { .. } => "REMOTE_REJECTED",
        RuntimeError::RemoteUnknown { .. } => "REMOTE_UNKNOWN",
        RuntimeError::Unavailable => "UNAVAILABLE_BEFORE_SEND",
        RuntimeError::StopUnconfirmed => "EXIT_UNCONFIRMED",
    }
}
async fn ready(db: &Database, session: &mut ClientSession) -> mongodb::error::Result<()> {
    if db
        .collection::<Document>("_schema_migrations")
        .find_one(doc! {"_id":"mcp","version":crate::storage::mcp_schema::VERSION})
        .session(&mut *session)
        .await?
        .is_none()
    {
        return Err(fail(Failure::NotReady));
    }
    shared::lock(db, "mcp", session).await
}
async fn observer_audit(
    db: &Database,
    session: &mut ClientSession,
    actor: ObjectId,
    target: ObjectId,
    action: &str,
) -> mongodb::error::Result<()> {
    db.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"administration","action":action,"actorId":actor,"targetId":target,"outcome":"success","observer":true,"createdAt":DateTime::now()})).session(session).await?;
    Ok(())
}
pub(super) async fn finish(
    db: &Database,
    config: &Config,
    action: &Action,
    status: &str,
    error: Option<&str>,
    reply: Option<&Reply>,
) -> Result<(), AppError> {
    let payload = reply
        .map(|v| serde_json::to_vec(&v.value).map_err(|_| AppError::NotReady))
        .transpose()?;
    if payload.as_ref().is_some_and(|v| v.len() > 512 * 1024) {
        return Err(AppError::PayloadTooLarge);
    }
    let sealed = payload
        .as_ref()
        .map(|bytes| vault::seal(config, &service::result_aad(action), bytes))
        .transpose()?;
    let hash = payload.as_ref().map(|bytes| service::hash(bytes));
    let size = payload.as_ref().map(|bytes| bytes.len() as i64);
    let kind = reply.map(|reply| {
        match reply.kind {
            ReplyKind::Complete => "complete",
            ReplyKind::InputRequired => "inputRequired",
            ReplyKind::Task => "task",
        }
        .to_owned()
    });
    let status = status.to_owned();
    let error = error.map(String::from);
    let now = DateTime::now();
    transaction::run(db,(db.clone(),action.clone(),sealed,hash,kind),|session,(db,action,sealed,hash,kind)|{let status=status.clone();let error=error.clone();async move{ready(db,session).await?;let stored=db.collection::<Action>("mcp_actions").find_one(doc!{"_id":action.id}).session(&mut *session).await?;let Some(stored)=stored else{return Ok(())};if !["pending","running"].contains(&stored.status.as_str()){return Ok(())}
if let Some(sealed)=sealed.as_ref(){db.collection::<ResultRecord>("mcp_results").update_one(doc!{"_id":action.id},doc!{"$setOnInsert":bson::serialize_to_document(&ResultRecord{id:action.id,actor_id:action.actor_id,owner_id:action.owner_id,connection_id:action.connection_id,sealed:sealed.clone(),expires_at:DateTime::from_millis(now.timestamp_millis()+30*24*60*60*1000)}) .map_err(|_|fail(Failure::NotReady))?}).upsert(true).session(&mut *session).await?;}db.collection::<Document>("mcp_actions").update_one(doc!{"_id":action.id,"status":{"$in":["pending","running"]}},doc!{"$set":{"status":status,"errorCode":error,"finishedAt":now,"resultKind":kind.clone(),"resultSize":size,"resultSha256":hash.clone()}}).session(&mut *session).await?;if action.kind!="call"{db.collection::<Document>("mcp_connections").update_one(doc!{"_id":action.connection_id,"controlActionId":action.id},doc!{"$set":{"controlActionId":null,"updatedAt":now}}).session(&mut *session).await?;}observer_audit(db,session,action.actor_id,action.id,"mcp.action.finish").await}.boxed()}).await
}
async fn finish_retry(
    db: &Database,
    config: &Config,
    action: &Action,
    status: &str,
    error: Option<&str>,
    reply: Option<&Reply>,
) {
    let mut delay = 100;
    loop {
        if finish(db, config, action, status, error, reply)
            .await
            .is_ok()
        {
            return;
        }
        if db
            .collection::<Document>("mcp_actions")
            .count_documents(doc! {"_id":action.id})
            .await
            .ok()
            == Some(0)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(delay)).await;
        delay = (delay * 2).min(2000);
    }
}
async fn heartbeat(
    db: &Database,
    config: &Arc<Config>,
    id: ObjectId,
    service: &str,
) -> Result<(), AppError> {
    transaction::run(db,(db.clone(),config.clone(),service.to_owned()),|session,(db,config,service)|async move{
        ready(db,session).await?;let now=DateTime::now();let until=lease();
        let instance=db.collection::<Instance>("mcp_instances").find_one(doc!{"_id":id,"serviceId":&*service,"releaseConfirmed":false,"state":{"$ne":"retired"},"leaseUntil":{"$gt":now}}).session(&mut *session).await?.ok_or_else(||fail(Failure::Conflict("运行实例租约已失效")))?;
        let connection=db.collection::<Document>("mcp_connections").find_one(doc!{"_id":instance.connection_id,"enabled":true,"deletedAt":null}).projection(doc!{"runtime":1,"controlActionId":1,"transport":1,"capabilityFingerprint":1}).session(&mut *session).await?.ok_or_else(||fail(Failure::Conflict("连接所有权已改变")))?;
        let transport=bson::deserialize_from_bson::<super::dto::Transport>(connection.get("transport").cloned().ok_or_else(||fail(Failure::NotReady))?).map_err(|_|fail(Failure::NotReady))?;
        let fingerprint=connection.get_str("capabilityFingerprint").map_err(|_|fail(Failure::NotReady))?;
        if !binding::plugin_current_tx(db,session,config,&transport,fingerprint).await?{return Err(fail(Failure::Conflict("插件 MCP 来源已改变")))}
        if instance.state=="reserved"{
            let action=connection.get_object_id("controlActionId").map_err(|_|fail(Failure::Conflict("重启意图已改变")))?;
            if db.collection::<Document>("mcp_actions").count_documents(doc!{"_id":action,"instanceId":id,"kind":"restart","status":{"$in":["pending","running"]}}).session(&mut *session).await?!=1{return Err(fail(Failure::Conflict("重启意图已改变")))}
        }else{
            let update=db.collection::<Document>("mcp_connections").update_one(doc!{"_id":instance.connection_id,"runtime.id":id,"runtime.serviceId":&*service,"runtime.releaseConfirmed":false,"runtime.state":{"$in":["starting","running"]}},doc!{"$set":{"runtime.leaseUntil":until,"runtime.updatedAt":now}}).session(&mut *session).await?;
            if update.matched_count!=1{return Err(fail(Failure::Conflict("连接所有权已改变")))}
        }
        let update=db.collection::<Document>("mcp_instances").update_one(doc!{"_id":id,"serviceId":&*service,"releaseConfirmed":false,"leaseUntil":{"$gt":now}},doc!{"$set":{"leaseUntil":until,"updatedAt":now}}).session(session).await?;
        if update.matched_count!=1{return Err(fail(Failure::Conflict("运行实例租约已失效")))}Ok(())
    }.boxed()).await
}
async fn set_instance(
    db: &Database,
    action: &Action,
    service: &str,
    state: &str,
    pid: Option<i64>,
    confirmed: bool,
    error: Option<&str>,
) -> Result<(), AppError> {
    let service = service.to_owned();
    let state = state.to_owned();
    let error = error.map(String::from);
    transaction::run(db,(db.clone(),action.clone()),|session,(db,action)|{let service=service.clone();let state=state.clone();let error=error.clone();async move{ready(db,session).await?;let id=action.instance_id.ok_or_else(||fail(Failure::NotReady))?;let Some(old)=db.collection::<Instance>("mcp_instances").find_one(doc!{"_id":id,"serviceId":&service}).session(&mut *session).await?else{return Ok(())};if old.state=="retired"{return Ok(())}let now=DateTime::now();let until=lease();db.collection::<Document>("mcp_instances").update_one(doc!{"_id":id,"serviceId":&service},doc!{"$set":{"state":&state,"pid":pid,"releaseConfirmed":confirmed,"leaseUntil":until,"updatedAt":now,"errorCode":error.clone()}}).session(&mut *session).await?;let meta=RuntimeMeta{id,transport_kind:old.transport_kind,service_id:service,state:state.clone(),pid,release_confirmed:confirmed,lease_until:until,updated_at:now,error_code:error};db.collection::<Document>("mcp_connections").update_one(doc!{"_id":action.connection_id,"runtime.id":id},doc!{"$set":{"runtime":bson::serialize_to_bson(&meta).map_err(|_|fail(Failure::NotReady))?,"updatedAt":now}}).session(&mut *session).await?;if confirmed{observer_audit(db,session,action.actor_id,id,if old.transport_kind==super::dto::TransportKind::Http{"mcp.http.release"}else{"mcp.instance.exit"}).await?;}Ok(())}.boxed()}).await
}
async fn terminal_retry(
    db: &Database,
    action: &Action,
    service: &str,
    state: &str,
    pid: Option<i64>,
    error: Option<&str>,
) {
    loop {
        if set_instance(db, action, service, state, pid, true, error)
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
pub(super) fn launch_action(
    state: &AppState,
    context: AuthContext,
    action: Action,
    old: Option<ObjectId>,
    capability: Option<Binding>,
) {
    let db = state.database.as_ref().unwrap().clone();
    let config = state.config.clone();
    let registry = Arc::downgrade(&state.mcp);
    if action.kind == "stop" {
        let entries = state.mcp.for_connection(action.connection_id);
        for entry in &entries {
            entry.stop.cancel();
        }
        tokio::spawn(async move {
            for entry in entries {
                entry.wait().await;
            }
            let confirmed = if let Some(id) = old {
                db.collection::<Instance>("mcp_instances")
                    .find_one(doc! {"_id":id})
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.release_confirmed || r.state == "retired")
            } else {
                true
            };
            finish_retry(
                &db,
                &config,
                &action,
                if confirmed { "succeeded" } else { "unknown" },
                if confirmed {
                    None
                } else {
                    Some("EXIT_UNCONFIRMED")
                },
                None,
            )
            .await;
        });
        return;
    }
    let Some(instance_id) = action.instance_id else {
        return;
    };
    let Some(entry) = state.mcp.insert(instance_id, action.connection_id) else {
        let service_id = state.mcp.service_id.clone();
        tokio::spawn(async move {
            terminal_retry(
                &db,
                &action,
                &service_id,
                "failed",
                None,
                Some("HOST_CAPACITY"),
            )
            .await;
            finish_retry(&db, &config, &action, "failed", Some("HOST_CAPACITY"), None).await;
        });
        return;
    };
    let service_id = state.mcp.service_id.clone();
    let capability = capability.unwrap();
    let old_entry = old.and_then(|id| state.mcp.get(id));
    tokio::spawn(async move {
        let lifetime = CancellationToken::new();
        let hb_db = db.clone();
        let hb_config = config.clone();
        let hb_service = service_id.clone();
        let hb_entry = entry.clone();
        let hb_end = lifetime.clone();
        let hb = tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(2));
            loop {
                tokio::select! {_ = hb_end.cancelled()=>break,_ = timer.tick()=>{if heartbeat(&hb_db,&hb_config,instance_id,&hb_service).await.is_err(){hb_entry.stop.cancel();break}}}
            }
        });
        if action.kind == "restart" {
            if let Some(old_entry) = old_entry {
                old_entry.stop.cancel();
                old_entry.wait().await;
            }
            let old_confirmed = if let Some(old) = old {
                db.collection::<Instance>("mcp_instances")
                    .find_one(doc! {"_id":old})
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|v| v.release_confirmed)
            } else {
                false
            };
            if !old_confirmed {
                entry.stop.cancel();
            } else {
                let _ = activate_reserved(&db, &context, &action, &service_id)
                    .await
                    .map_err(|_| entry.stop.cancel());
            }
        }
        let outcome = match &capability {
            Binding::Stdio(capability) | Binding::Plugin(PluginProgram { capability, .. }) => {
                run_program(
                    &db,
                    &config,
                    &entry,
                    &context,
                    &action,
                    capability,
                    &service_id,
                )
                .await
            }
            Binding::Http(capability) => {
                run_http(
                    &db,
                    &config,
                    &entry,
                    &context,
                    &action,
                    capability,
                    &service_id,
                )
                .await
            }
        };
        lifetime.cancel();
        let _ = hb.await;
        let (state, pid, error) = outcome;
        terminal_retry(&db, &action, &service_id, &state, pid, error.as_deref()).await;
        let stored = db
            .collection::<Action>("mcp_actions")
            .find_one(doc! {"_id":action.id})
            .await
            .ok()
            .flatten();
        if stored.is_some_and(|v| ["pending", "running"].contains(&v.status.as_str())) {
            finish_retry(
                &db,
                &config,
                &action,
                if entry.stop.is_cancelled() {
                    "cancelled"
                } else {
                    "failed"
                },
                error.as_deref().or(Some("PROCESS_ENDED")),
                None,
            )
            .await;
        }
        entry.finished();
        if let Some(registry) = registry.upgrade() {
            registry.remove(instance_id);
        }
    });
}
async fn activate_reserved(
    db: &Database,
    context: &AuthContext,
    action: &Action,
    service_id: &str,
) -> Result<(), AppError> {
    transaction::run(db,(db.clone(),context.clone(),action.clone(),service_id.to_owned()),|session,(db,context,action,service)|async move{let actor=owned::fresh(db,session,context,policy("mcp.write"),true).await?;let row=connection_tx(db,session,&actor,action.connection_id,true).await?;if row.version!=action.connection_version||row.control_action_id!=Some(action.id){return Err(fail(Failure::Conflict("重启意图已改变")))}let id=action.instance_id.unwrap();let now=DateTime::now();let meta=RuntimeMeta{id,transport_kind:row.transport.kind(),service_id:service.clone(),state:"starting".into(),pid:None,release_confirmed:false,lease_until:lease(),updated_at:now,error_code:None};db.collection::<Document>("mcp_connections").update_one(doc!{"_id":row.id,"controlActionId":action.id},doc!{"$set":{"runtime":bson::serialize_to_bson(&meta).map_err(|_|fail(Failure::NotReady))?}}).session(session).await?;Ok(())}.boxed()).await
}
async fn run_program(
    db: &Database,
    config: &Config,
    entry: &Arc<Entry>,
    context: &AuthContext,
    action: &Action,
    capability: &Capability,
    service_id: &str,
) -> (String, Option<i64>, Option<String>) {
    if entry.stop.is_cancelled() {
        return (
            "stopped".into(),
            None,
            Some("CANCELLED_BEFORE_START".into()),
        );
    }
    if authorize_start(db, config, context, action).await.is_err() {
        return ("failed".into(), None, Some("AUTHORITY_CHANGED".into()));
    }
    let mut process = match Process::spawn(capability.spec(), capability.limits()) {
        Ok(process) => process,
        Err(error) => return ("failed".into(), None, Some(code(&error).into())),
    };
    let pid = Some(i64::from(process.state().pid));
    let recorded = tokio::select! {_ = entry.stop.cancelled()=>false,result = set_instance(db,action,service_id,"starting",pid,false,None)=>result.is_ok()};
    if !recorded {
        entry.stop.cancel();
    }
    let connected =
        tokio::select! {_ = entry.stop.cancelled()=>None,result = process.connect()=>Some(result)};
    let client = match connected {
        Some(Ok(client)) => Arc::new(client),
        failure => {
            let error = failure
                .and_then(Result::err)
                .map(|error| code(&error).to_owned())
                .unwrap_or_else(|| "START_CANCELLED".into());
            stop_confirmed(&process).await;
            return (
                if entry.stop.is_cancelled() {
                    "stopped"
                } else {
                    "failed"
                }
                .into(),
                pid,
                Some(error),
            );
        }
    };
    let tools = match client.tools(entry.stop.child_token()).await {
        Ok(tools) => tools
            .into_iter()
            .filter(|tool| {
                tool["name"].as_str().is_some_and(|name| {
                    capability
                        .allowed_tools
                        .iter()
                        .any(|allowed| allowed == name)
                })
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            stop_confirmed(&process).await;
            client.close().await;
            return ("failed".into(), pid, Some(code(&error).into()));
        }
    };
    *entry.client.write().await = Some(client.clone());
    let publication = tokio::select! {_ = entry.stop.cancelled()=>false,result = publish_running(db,config,context,action,service_id,pid,tools)=>result.is_ok()};
    let committed = if publication {
        true
    } else {
        tokio::select! {_ = entry.stop.cancelled()=>false,result=running_committed(db,action,service_id)=>result.unwrap_or(false)}
    };
    if !committed {
        entry.stop.cancel();
        stop_confirmed(&process).await;
        client.close().await;
        return ("failed".into(), pid, Some("START_COMMIT_REJECTED".into()));
    }
    let mut observed = process.subscribe();
    loop {
        if observed.borrow().phase == Phase::Exited {
            break;
        }
        tokio::select! {_ = entry.stop.cancelled()=>{stop_confirmed(&process).await;break},changed = observed.changed()=>{if changed.is_err(){entry.stop.cancel();stop_confirmed(&process).await;break}}}
    }
    *entry.client.write().await = None;
    client.close().await;
    let state = if entry.stop.is_cancelled() {
        "stopped"
    } else {
        "failed"
    };
    let error = if state == "stopped" {
        None
    } else {
        Some("PROCESS_EXITED".into())
    };
    (state.into(), pid, error)
}
async fn running_committed(
    db: &Database,
    action: &Action,
    service_id: &str,
) -> Result<bool, AppError> {
    transaction::run_snapshot(db,(db.clone(),action.clone(),service_id.to_owned()),|session,(db,action,service)|async move{let row=db.collection::<Action>("mcp_actions").find_one(doc!{"_id":action.id,"status":"succeeded","instanceId":action.instance_id}).session(&mut *session).await?;if row.is_none(){return Ok(false)}let connection=db.collection::<Document>("mcp_connections").find_one(doc!{"_id":action.connection_id,"version":action.connection_version,"runtime.id":action.instance_id,"runtime.serviceId":&*service,"runtime.state":"running","controlActionId":null,"enabled":true,"deletedAt":null}).session(session).await?;Ok(connection.is_some())}.boxed()).await
}
async fn run_http(
    db: &Database,
    config: &Config,
    entry: &Arc<Entry>,
    context: &AuthContext,
    action: &Action,
    capability: &HttpBinding,
    service_id: &str,
) -> (String, Option<i64>, Option<String>) {
    if entry.stop.is_cancelled() {
        return (
            "stopped".into(),
            None,
            Some("CANCELLED_BEFORE_START".into()),
        );
    }
    if authorize_start(db, config, context, action).await.is_err() {
        return ("failed".into(), None, Some("AUTHORITY_CHANGED".into()));
    }
    let spec = tokio::select! {_=entry.stop.cancelled()=>return ("stopped".into(),None,Some("START_CANCELLED".into())),value=capability.spec(db,config)=>match value{Ok(value)=>value,Err(_)=>return ("failed".into(),None,Some("HTTP_CONFIGURATION_UNAVAILABLE".into()))}};
    let connected = tokio::select! {_=entry.stop.cancelled()=>None,value=tri_mcp_runtime::http::connect(spec,tri_mcp_runtime::session::Limits::default())=>Some(value)};
    let client = match connected {
        Some(Ok(client)) => Arc::new(client),
        value => {
            return (
                if entry.stop.is_cancelled() {
                    "stopped"
                } else {
                    "failed"
                }
                .into(),
                None,
                Some(
                    value
                        .and_then(Result::err)
                        .map(|value| code(&value))
                        .unwrap_or("START_CANCELLED")
                        .into(),
                ),
            );
        }
    };
    let tools = match client.tools(entry.stop.child_token()).await {
        Ok(tools) => tools
            .into_iter()
            .filter(|tool| {
                tool["name"].as_str().is_some_and(|name| {
                    capability.allowed_tools().iter().any(|value| value == name)
                })
            })
            .collect::<Vec<_>>(),
        Err(error) => {
            client.close().await;
            return ("failed".into(), None, Some(code(&error).into()));
        }
    };
    *entry.client.write().await = Some(client.clone());
    let publication = tokio::select! {_=entry.stop.cancelled()=>false,result=publish_running(db,config,context,action,service_id,None,tools)=>result.is_ok()};
    let committed = if publication {
        true
    } else {
        tokio::select! {_=entry.stop.cancelled()=>false,result=running_committed(db,action,service_id)=>result.unwrap_or(false)}
    };
    if !committed {
        entry.stop.cancel();
        *entry.client.write().await = None;
        client.close().await;
        return ("failed".into(), None, Some("START_COMMIT_REJECTED".into()));
    }
    let mut timer = tokio::time::interval(Duration::from_secs(2));
    let mut reason = None;
    loop {
        tokio::select! {_=entry.stop.cancelled()=>break,_=timer.tick()=>{
            if client.is_closed(){reason=Some("HTTP_DISCONNECTED".into());break}
            if !capability.current(db,config).await{reason=Some("HTTP_BINDING_CHANGED".into());entry.stop.cancel();break}
        }}
    }
    *entry.client.write().await = None;
    client.close().await;
    (
        if entry.stop.is_cancelled() {
            "stopped"
        } else {
            "disconnected"
        }
        .into(),
        None,
        reason,
    )
}
async fn stop_confirmed(process: &Process) {
    let _ = process.stop().await;
    let mut state = process.subscribe();
    loop {
        if state.borrow().phase == Phase::Exited {
            return;
        }
        if state.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
pub(super) async fn authorize_start(
    db: &Database,
    config: &Config,
    context: &AuthContext,
    action: &Action,
) -> Result<(), AppError> {
    transaction::run_snapshot(
        db,
        (db.clone(), config.clone(), context.clone(), action.clone()),
        |session, (db, config, context, action)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.write"), false).await?;
                if !actor.can("mcp.write") {
                    return Err(fail(Failure::Forbidden));
                }
                let row = connection_tx(db, session, &actor, action.connection_id, true).await?;
                binding::ensure(db, session, config, &row, &actor).await?;
                if !row.enabled
                    || row.version != action.connection_version
                    || row.control_action_id != Some(action.id)
                {
                    return Err(fail(Failure::Conflict("启动意图已改变")));
                }
                Ok(())
            }
            .boxed()
        },
    )
    .await
}
async fn publish_running(
    db: &Database,
    config: &Config,
    context: &AuthContext,
    action: &Action,
    service_id: &str,
    pid: Option<i64>,
    tools: Vec<Value>,
) -> Result<(), AppError> {
    let json = serde_json::to_string(&tools).map_err(|_| AppError::NotReady)?;
    transaction::run(db,(db.clone(),config.clone(),context.clone(),action.clone(),service_id.to_owned(),json),|session,(db,config,context,action,service,json)|async move{let actor=owned::fresh(db,session,context,policy("mcp.write"),true).await?;let row=connection_tx(db,session,&actor,action.connection_id,true).await?;binding::ensure(db,session,config,&row,&actor).await?;if !row.enabled||row.version!=action.connection_version||row.control_action_id!=Some(action.id)||row.runtime.as_ref().map(|r|r.id)!=action.instance_id{return Err(fail(Failure::Conflict("启动意图已改变")))}let now=DateTime::now();let until=lease();let id=action.instance_id.unwrap();db.collection::<Document>("mcp_instances").update_one(doc!{"_id":id,"serviceId":&*service,"releaseConfirmed":false,"state":{"$ne":"retired"}},doc!{"$set":{"state":"running","pid":pid,"toolsJson":&*json,"updatedAt":now,"leaseUntil":until}}).session(&mut *session).await?;db.collection::<Document>("mcp_connections").update_one(doc!{"_id":row.id,"controlActionId":action.id},doc!{"$set":{"runtime.state":"running","runtime.pid":pid,"runtime.updatedAt":now,"runtime.leaseUntil":until,"controlActionId":null}}).session(&mut *session).await?;db.collection::<Document>("mcp_actions").update_one(doc!{"_id":action.id,"status":"pending"},doc!{"$set":{"status":"succeeded","finishedAt":now}}).session(&mut *session).await?;shared::audit(db,&actor,"mcp.running",id,session).await}.boxed()).await
}
pub(super) fn launch_call(
    state: &AppState,
    context: AuthContext,
    action: Action,
    arguments: Value,
) {
    let db = state.database.as_ref().unwrap().clone();
    let config = state.config.clone();
    let entry = action.instance_id.and_then(|id| state.mcp.get(id));
    tokio::spawn(async move {
        let Some(entry) = entry else {
            finish_retry(
                &db,
                &config,
                &action,
                "failed",
                Some("NO_OWNED_INSTANCE"),
                None,
            )
            .await;
            return;
        };
        let cancel = entry.stop.child_token();
        entry
            .calls
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .insert(action.id, cancel.clone());
        let permitted = authorize_call(&db, &config, &context, &action).await;
        if permitted.is_err() {
            entry
                .calls
                .lock()
                .unwrap_or_else(|v| v.into_inner())
                .remove(&action.id);
            let cancelled = db
                .collection::<Action>("mcp_actions")
                .find_one(doc! {"_id":action.id})
                .await
                .ok()
                .flatten()
                .is_some_and(|v| v.cancel_requested);
            finish_retry(
                &db,
                &config,
                &action,
                if cancelled { "cancelled" } else { "failed" },
                Some(if cancelled {
                    "CANCELLED_BEFORE_SEND"
                } else {
                    "DISPATCH_REJECTED"
                }),
                None,
            )
            .await;
            return;
        }
        let client = entry.client.read().await.clone();
        let result = if let Some(client) = client {
            client
                .call(action.name.as_deref().unwrap(), arguments, cancel)
                .await
        } else {
            Err(RuntimeError::Unavailable)
        };
        entry
            .calls
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .remove(&action.id);
        match result {
            Ok(reply) => {
                let status = match reply.kind {
                    ReplyKind::Complete => {
                        if reply.value["isError"] == true {
                            "failed"
                        } else {
                            "succeeded"
                        }
                    }
                    ReplyKind::InputRequired => "inputRequired",
                    ReplyKind::Task => "remoteTask",
                };
                finish_retry(&db, &config, &action, status, None, Some(&reply)).await;
            }
            Err(error) => {
                let status = match error {
                    RuntimeError::CancelledBeforeSend => "cancelled",
                    RuntimeError::BusyBeforeSend
                    | RuntimeError::InvalidInput
                    | RuntimeError::Unavailable
                    | RuntimeError::RemoteRejected { .. } => "failed",
                    _ => "unknown",
                };
                finish_retry(&db, &config, &action, status, Some(code(&error)), None).await;
            }
        }
    });
}
async fn refresh_access(
    db: &Database,
    config: &Config,
    context: &AuthContext,
    action: &Action,
    service: &str,
    tools: Option<Vec<Value>>,
) -> Result<(), AppError> {
    transaction::run(db,(db.clone(),config.clone(),context.clone(),action.clone(),service.to_owned(),tools),|session,(db,config,context,action,service,tools)|async move{
        let actor=owned::fresh(db,session,context,policy("mcp.write"),true).await?;
        let row=connection_tx(db,session,&actor,action.connection_id,true).await?;
        let binding=binding::ensure(db,session,config,&row,&actor).await?;
        if !row.enabled || row.version!=action.connection_version || row.control_action_id!=Some(action.id) || row.runtime.as_ref().is_none_or(|runtime|Some(runtime.id)!=action.instance_id || runtime.service_id!=*service || runtime.state!="running" || runtime.release_confirmed || runtime.lease_until<DateTime::now()){return Err(fail(Failure::Conflict("工具刷新实例已改变")))}
        let stored=db.collection::<Action>("mcp_actions").find_one(doc!{"_id":action.id,"status":{"$in":["pending","running"]},"cancelRequested":false}).session(&mut *session).await?.ok_or_else(||fail(Failure::Conflict("工具刷新操作已改变")))?;
        let Some(tools)=tools else{db.collection::<Document>("mcp_actions").update_one(doc!{"_id":stored.id},doc!{"$set":{"status":"running"}}).session(session).await?;return Ok(())};
        let tools=tools.iter().filter(|tool|tool["name"].as_str().is_some_and(|name|binding.allowed_tools().iter().any(|allowed|allowed==name))).cloned().collect::<Vec<_>>();
        let json=serde_json::to_string(&tools).map_err(|_|fail(Failure::NotReady))?;let now=DateTime::now();
        let updated=db.collection::<Document>("mcp_instances").update_one(doc!{"_id":action.instance_id,"serviceId":&*service,"state":"running","releaseConfirmed":false,"leaseUntil":{"$gt":now}},doc!{"$set":{"toolsJson":json,"updatedAt":now}}).session(&mut *session).await?;
        if updated.matched_count!=1{return Err(fail(Failure::Conflict("工具刷新实例已释放")))}
        db.collection::<Document>("mcp_connections").update_one(doc!{"_id":row.id,"controlActionId":action.id},doc!{"$set":{"controlActionId":null,"runtime.updatedAt":now,"updatedAt":now}}).session(&mut *session).await?;
        db.collection::<Document>("mcp_actions").update_one(doc!{"_id":action.id},doc!{"$set":{"status":"succeeded","finishedAt":now}}).session(&mut *session).await?;
        shared::audit(db,&actor,"mcp.tools.refresh",action.instance_id.unwrap(),session).await
    }.boxed()).await
}
pub(super) fn launch_refresh(state: &AppState, context: AuthContext, action: Action) {
    let db = state.database.as_ref().unwrap().clone();
    let config = state.config.clone();
    let service = state.mcp.service_id.clone();
    let entry = action.instance_id.and_then(|id| state.mcp.get(id));
    tokio::spawn(async move {
        let Some(entry) = entry else {
            finish_retry(
                &db,
                &config,
                &action,
                "failed",
                Some("INSTANCE_UNAVAILABLE_BEFORE_SEND"),
                None,
            )
            .await;
            return;
        };
        let Some(client) = entry.client.read().await.clone() else {
            finish_retry(
                &db,
                &config,
                &action,
                "failed",
                Some("INSTANCE_UNAVAILABLE_BEFORE_SEND"),
                None,
            )
            .await;
            return;
        };
        let cancel = entry.stop.child_token();
        entry
            .calls
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .insert(action.id, cancel.clone());
        let result = if refresh_access(&db, &config, &context, &action, &service, None)
            .await
            .is_err()
        {
            Err(None)
        } else {
            match client.tools(cancel).await {
                Ok(tools) => refresh_access(&db, &config, &context, &action, &service, Some(tools))
                    .await
                    .map_err(|_| None),
                Err(error) => Err(Some(error)),
            }
        };
        entry
            .calls
            .lock()
            .unwrap_or_else(|value| value.into_inner())
            .remove(&action.id);
        if let Err(error) = result {
            let status = match &error {
                Some(
                    RuntimeError::CancelledUnknown
                    | RuntimeError::TimeoutUnknown
                    | RuntimeError::TransportUnknown
                    | RuntimeError::RemoteUnknown { .. },
                ) => "unknown",
                _ if entry.stop.is_cancelled() => "cancelled",
                _ => "failed",
            };
            finish_retry(
                &db,
                &config,
                &action,
                status,
                Some(
                    error
                        .as_ref()
                        .map(code)
                        .unwrap_or("REFRESH_COMMIT_REJECTED"),
                ),
                None,
            )
            .await;
        }
    });
}
async fn authorize_call(
    db: &Database,
    config: &Config,
    context: &AuthContext,
    action: &Action,
) -> Result<(), AppError> {
    transaction::run(
        db,
        (db.clone(), config.clone(), context.clone(), action.clone()),
        |session, (db, config, context, action)| {
            async move {
                let actor = owned::fresh(db, session, context, policy("mcp.invoke"), true).await?;
                let row = connection_tx(db, session, &actor, action.connection_id, true).await?;
                binding::ensure(db, session, config, &row, &actor).await?;
                let stored = db
                    .collection::<Action>("mcp_actions")
                    .find_one(doc! {"_id":action.id,"status":"pending"})
                    .session(&mut *session)
                    .await?
                    .ok_or_else(|| fail(Failure::Conflict("调用状态已改变")))?;
                if stored.cancel_requested {
                    return Err(fail(Failure::Conflict("调用已经取消")));
                }
                if !row.enabled
                    || row.version != action.connection_version
                    || row.control_action_id.is_some()
                    || row.runtime.as_ref().is_none_or(|r| {
                        Some(r.id) != action.instance_id
                            || r.state != "running"
                            || r.lease_until < DateTime::now()
                    })
                {
                    return Err(fail(Failure::Conflict("实例不接受调用")));
                }
                db.collection::<Document>("mcp_actions")
                    .update_one(
                        doc! {"_id":action.id,"status":"pending"},
                        doc! {"$set":{"status":"running"}},
                    )
                    .session(session)
                    .await?;
                Ok(())
            }
            .boxed()
        },
    )
    .await
}
