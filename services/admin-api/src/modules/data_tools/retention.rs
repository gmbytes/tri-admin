use crate::{
    common::{
        error::AppError,
        transaction::{self, Failure, fail},
    },
    state::AppState,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
    options::ReturnDocument,
};
use serde::{Deserialize, Serialize};
#[derive(Default, Serialize)]
pub struct MaintenanceReport {
    pub scanned: u64,
    pub redacted: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetentionMeta {
    #[serde(rename = "_id")]
    id: ObjectId,
    actor_id: ObjectId,
    row_count: i64,
}
pub async fn maintenance_once(database: &Database) -> Result<MaintenanceReport, AppError> {
    if !crate::storage::schema::is_initialized(database).await? {
        return Err(AppError::NotReady);
    }
    let cutoff = DateTime::now();
    let candidates = database
        .collection::<Document>("data_imports")
        .find(doc! {"payloadState":"full","expiresAt":{"$lte":cutoff}})
        .projection(doc! {"_id":1})
        .sort(doc! {"expiresAt":1,"_id":1})
        .limit(100)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let mut report = MaintenanceReport {
        scanned: candidates.len() as u64,
        redacted: 0,
    };
    for candidate in candidates {
        let id = candidate
            .get_object_id("_id")
            .map_err(|_| AppError::Internal)?;
        let changed=transaction::run(database,(database.clone(),cutoff),|session,(database,cutoff)|async move{
            let ready=doc!{"$eq":["$$row.status","ready"]};
            let pipeline=vec![doc!{"$set":{"rows":{"$map":{"input":"$rows","as":"row","in":{"index":"$$row.index","key":"","status":{"$cond":[ready.clone(),"expired","$$row.status"]},"error":{"$cond":[ready,"PREVIEW_EXPIRED","$$row.error"]},"targetId":"$$row.targetId","valuesJson":null,"parentId":null}}},"payloadState":"redacted","redactedAt":*cutoff,"errorCount":{"$add":["$errorCount","$readyCount"]},"expiredCount":{"$add":["$expiredCount","$readyCount"]},"readyCount":0i64}}];
            let previous=database.collection::<RetentionMeta>("data_imports").find_one_and_update(doc!{"_id":id,"payloadState":"full","expiresAt":{"$lte":*cutoff}},pipeline).projection(doc!{"actorId":1,"rowCount":1}).return_document(ReturnDocument::Before).session(&mut *session).await?;
            let Some(previous)=previous else{return Ok(false)};
            if !(0..=2000).contains(&previous.row_count){return Err(fail(Failure::NotReady));}
            database.collection::<Document>("audit_events").insert_one(crate::common::observability::event(doc!{"category":"maintenance","action":"dataTools.import.redact","actorId":null,"subjectActorId":previous.actor_id,"targetId":previous.id,"rowCount":previous.row_count,"outcome":"success","createdAt":*cutoff})).session(session).await?;
            Ok(true)
        }.boxed()).await?;
        report.redacted += u64::from(changed);
    }
    Ok(report)
}
pub fn start_service(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {_=state.data_tool_shutdown.cancelled()=>break,_=ticker.tick()=>{}}
            let Some(database) = &state.database else {
                continue;
            };
            tokio::select! {_=state.data_tool_shutdown.cancelled()=>break,result=maintenance_once(database)=>{if result.is_err(){tracing::warn!("data tool retention deferred");}}}
        }
    })
}
pub fn stop_service(state: &AppState) {
    state.data_tool_shutdown.cancel();
}
