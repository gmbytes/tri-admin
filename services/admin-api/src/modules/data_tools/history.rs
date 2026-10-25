use super::{
    models::{ImportMeta, ImportSummary},
    registry,
};
use crate::{
    common::{
        error::AppError,
        pagination::{Page, PageInput},
    },
    modules::rbac::shared::Actor,
};
use futures_util::{FutureExt, TryStreamExt};
use mongodb::{
    Database,
    bson::{DateTime, Document, doc},
};
use serde::Deserialize;
fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryQuery {
    #[serde(default = "first")]
    pub page: u64,
    #[serde(default = "size")]
    pub page_size: u64,
    pub resource: Option<String>,
    pub status: Option<HistoryStatus>,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryStatus {
    Pending,
    Completed,
    Partial,
    Failed,
    Expired,
}
fn status_filter(status: HistoryStatus, now: DateTime) -> Document {
    match status {
        HistoryStatus::Pending => {
            doc! {"readyCount":{"$gt":0},"expiresAt":{"$gt":now},"payloadState":"full"}
        }
        HistoryStatus::Completed => doc! {"readyCount":0,"errorCount":0},
        HistoryStatus::Partial => {
            doc! {"appliedCount":{"$gt":0},"$and":[{"$or":[{"readyCount":0},{"expiresAt":{"$lte":now}}]},{"$or":[{"errorCount":{"$gt":0}},{"readyCount":{"$gt":0}}]}]}
        }
        HistoryStatus::Failed => {
            doc! {"appliedCount":0,"readyCount":0,"errorCount":{"$gt":0},"expiredCount":0}
        }
        HistoryStatus::Expired => {
            doc! {"appliedCount":0,"$or":[{"expiredCount":{"$gt":0}},{"readyCount":{"$gt":0},"expiresAt":{"$lte":now}}]}
        }
    }
}
pub async fn list(
    database: &Database,
    actor: &Actor,
    input: HistoryQuery,
) -> Result<Page<ImportSummary>, AppError> {
    let page = PageInput {
        page: input.page,
        page_size: input.page_size,
        search: None,
        sort: None,
    };
    let skip = page.validate()?;
    let resources = if let Some(key) = &input.resource {
        let definition = registry::resource(key, actor, false)?;
        if !definition.importable {
            return Err(AppError::Validation("该资源没有导入历史".into()));
        }
        vec![definition.key]
    } else {
        registry::RESOURCES
            .iter()
            .filter(|r| r.importable && actor.can(r.read_permission))
            .map(|r| r.key)
            .collect()
    };
    let now = DateTime::now();
    let mut filter = doc! {"actorId":actor.id,"resource":{"$in":resources}};
    if let Some(status) = input.status {
        filter.extend(status_filter(status, now));
    }
    let limit = page.page_size as i64;
    let (total,records)=crate::common::transaction::run_snapshot(database,(database.clone(),filter),|session,(database,filter)|async move{
        let total=database.collection::<Document>("data_imports").count_documents(filter.clone()).session(&mut *session).await?;
        let mut cursor=database.collection::<ImportMeta>("data_imports").find(filter.clone()).projection(doc!{"requestId":1,"resource":1,"createdAt":1,"expiresAt":1,"payloadState":1,"redactedAt":1,"rowCount":1,"readyCount":1,"appliedCount":1,"errorCount":1,"expiredCount":1}).sort(doc!{"createdAt":-1,"_id":-1}).skip(skip).limit(limit).session(&mut *session).await?;
        let records=cursor.stream(session).try_collect::<Vec<_>>().await?;
        Ok((total,records))
    }.boxed()).await?;
    let rows = records
        .into_iter()
        .map(|record| record.summary(now))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page::new(rows, &page, total))
}
