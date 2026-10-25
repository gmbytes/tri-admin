use super::super::{formats::Cell, query, registry};
use super::{authorization, models::JobRecord};
use crate::{common::error::AppError, modules::rbac::shared::Actor};
use mongodb::{
    Database,
    bson::{Document, doc, oid::ObjectId},
};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

pub struct Row {
    pub id: ObjectId,
    pub cells: Vec<Cell>,
}
pub enum Message {
    Row(Row),
    Finish,
}
pub fn cells(resource: &str, fields: &[String], record: &Document) -> Result<Vec<Cell>, AppError> {
    fields
        .iter()
        .map(|field| {
            Ok(match field.as_str() {
                "id" => Cell::Text(
                    record
                        .get_object_id("_id")
                        .map_err(|_| AppError::Internal)?
                        .to_hex(),
                ),
                "version" => {
                    Cell::Number(record.get_i64("version").map_err(|_| AppError::Internal)? as f64)
                }
                "sort" => {
                    Cell::Number(record.get_i32("sort").map_err(|_| AppError::Internal)? as f64)
                }
                "enabled" => {
                    Cell::Boolean(record.get_bool("enabled").map_err(|_| AppError::Internal)?)
                }
                "value" if resource == "parameters" => {
                    let value: serde_json::Value = serde_json::from_str(
                        record
                            .get_str("valueJson")
                            .map_err(|_| AppError::Internal)?,
                    )
                    .map_err(|_| AppError::Internal)?;
                    match record
                        .get_str("valueType")
                        .map_err(|_| AppError::Internal)?
                    {
                        "number" => Cell::Number(value.as_f64().ok_or(AppError::Internal)?),
                        "boolean" => Cell::Boolean(value.as_bool().ok_or(AppError::Internal)?),
                        "string" => Cell::Text(value.as_str().ok_or(AppError::Internal)?.into()),
                        "json" => Cell::Text(value.to_string()),
                        _ => return Err(AppError::Internal),
                    }
                }
                _ => Cell::Text(
                    record
                        .get_str(field)
                        .map_err(|_| AppError::Internal)?
                        .into(),
                ),
            })
        })
        .collect()
}
pub async fn stream(
    database: &Database,
    job: &JobRecord,
    actor: &Actor,
    sender: Sender<Message>,
    cancel: CancellationToken,
) -> Result<u32, AppError> {
    registry::resource(&job.spec.resource, actor, false)?;
    let fields = registry::fields(
        &job.spec
            .columns
            .iter()
            .map(|column| column.field.clone())
            .collect::<Vec<_>>(),
        registry::definition(&job.spec.resource)?,
    )?;
    let plan = query::plan(
        &job.spec.resource,
        &job.spec.search,
        &job.spec.filters,
        job.spec.sort.as_ref(),
    )?;
    let mut session = database.client().start_session().snapshot(true).await?;
    let resource = job.spec.resource.as_str();
    let mut pipeline = if resource == "dictionaryItems" {
        let mut parent_sort = Document::new();
        for (key, value) in plan.sort {
            parent_sort.insert(
                if key == "key" {
                    "dictionaryKey".to_owned()
                } else {
                    key
                },
                value,
            );
        }
        vec![
            doc! {"$match":plan.rows},
            doc! {"$lookup":{"from":"dictionaries","localField":"dictionaryId","foreignField":"_id","pipeline":[{"$match":plan.parent},{"$limit":1}],"as":"parent"}},
            doc! {"$unwind":"$parent"},
            doc! {"$project":{"_id":1,"dictionaryKey":"$parent.key","label":1,"value":1,"sort":1,"enabled":1,"createdAt":1,"version":1}},
            doc! {"$sort":parent_sort},
        ]
    } else {
        let filter = if resource == "users" {
            doc! {"$and":[plan.rows,authorization::user_filter(job,actor)]}
        } else {
            plan.rows
        };
        let projection = match resource {
            "users" => doc! {"username":1,"displayName":1,"status":1,"version":1},
            "dictionaries" => doc! {"name":1,"key":1,"description":1,"enabled":1},
            _ => doc! {"name":1,"key":1,"group":1,"description":1,"valueType":1,"valueJson":1},
        };
        vec![
            doc! {"$match":filter},
            doc! {"$sort":plan.sort},
            doc! {"$project":projection},
        ]
    };
    pipeline.push(doc! {"$limit":job.meta.max_rows as i64+1});
    let collection = if resource == "dictionaryItems" {
        "dictionary_items"
    } else {
        resource
    };
    let mut cursor = database
        .collection::<Document>(collection)
        .aggregate(pipeline)
        .allow_disk_use(true)
        .batch_size(64)
        .session(&mut session)
        .await?;
    let mut count = 0u32;
    loop {
        let next = tokio::select! {biased;_=cancel.cancelled()=>return Err(AppError::Conflict("导出已停止".into())),row=cursor.next(&mut session)=>row.transpose()?};
        let Some(record) = next else { break };
        count += 1;
        if count > job.meta.max_rows {
            return Err(AppError::Validation("ROW_LIMIT".into()));
        }
        let row = Row {
            id: record
                .get_object_id("_id")
                .map_err(|_| AppError::Internal)?,
            cells: cells(resource, &fields, &record)?,
        };
        tokio::select! {biased;_=cancel.cancelled()=>return Err(AppError::Conflict("导出已停止".into())),result=sender.send(Message::Row(row))=>result.map_err(|_|AppError::Conflict("导出写入已停止".into()))?};
    }
    Ok(count)
}
