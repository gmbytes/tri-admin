use super::{
    formats::{self, Cell, Format, Table},
    models::ExportInput,
    query, registry,
};
use crate::{common::error::AppError, modules::rbac::shared::Actor};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::TryStreamExt;
use mongodb::{
    Database,
    bson::{Document, doc},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    name: String,
    mime_type: &'static str,
    size: usize,
    sha256: String,
    content_base64: String,
    row_count: usize,
}
pub fn download(format: Format, resource: &str, table: Table) -> Result<Download, AppError> {
    let row_count = table.rows.len();
    let bytes = formats::encode(format, &table)?;
    Ok(Download {
        name: format!(
            "tri-admin-{resource}.{}",
            if format == Format::Csv { "csv" } else { "xlsx" }
        ),
        mime_type: if format == Format::Csv {
            "text/csv;charset=utf-8"
        } else {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        },
        size: bytes.len(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        content_base64: STANDARD.encode(bytes),
        row_count,
    })
}
pub async fn export(
    database: &Database,
    actor: &Actor,
    input: &ExportInput,
) -> Result<Table, AppError> {
    export_limited(database, actor, input, formats::MAX_ROWS).await
}
pub async fn export_limited(
    database: &Database,
    actor: &Actor,
    input: &ExportInput,
    limit: usize,
) -> Result<Table, AppError> {
    if !(1..=formats::MAX_ROWS).contains(&limit) {
        return Err(AppError::Validation("同步导出行数上限无效".into()));
    }
    let resource = registry::resource(&input.resource, actor, false)?;
    let fields = registry::fields(&input.fields, resource)?;
    let plan = query::plan(
        &input.resource,
        &input.search,
        &input.filters,
        input.sort.as_ref(),
    )?;
    if resource.key == "dictionaryItems" {
        return item_export(database, fields, plan, limit).await;
    }
    let filter = if resource.key == "users" {
        doc! {"$and":[plan.rows,actor.user_filter()]}
    } else {
        plan.rows
    };
    // Explicit projection: credentials are never read into an export buffer.
    let projection = if resource.key == "users" {
        doc! {"username":1,"displayName":1,"status":1,"version":1}
    } else if resource.key == "dictionaries" {
        doc! {"name":1,"key":1,"description":1,"enabled":1}
    } else {
        doc! {"name":1,"key":1,"group":1,"description":1,"valueType":1,"valueJson":1}
    };
    let records = database
        .collection::<Document>(resource.key)
        .find(filter)
        .projection(projection)
        .sort(plan.sort)
        .limit((limit + 1) as i64)
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    if records.len() > limit {
        return Err(AppError::Validation(
            "导出超过本次行数上限，请缩小筛选".into(),
        ));
    }
    let rows = records
        .into_iter()
        .map(|record| {
            fields
                .iter()
                .map(|field| match field.as_str() {
                    "id" => record
                        .get_object_id("_id")
                        .map(|v| Cell::Text(v.to_hex()))
                        .map_err(|_| AppError::Internal),
                    "version" => record
                        .get_i64("version")
                        .map(|v| Cell::Number(v as f64))
                        .map_err(|_| AppError::Internal),
                    "enabled" => record
                        .get_bool("enabled")
                        .map(Cell::Boolean)
                        .map_err(|_| AppError::Internal),
                    "value" => {
                        let value: serde_json::Value = serde_json::from_str(
                            record
                                .get_str("valueJson")
                                .map_err(|_| AppError::Internal)?,
                        )
                        .map_err(|_| AppError::Internal)?;
                        Ok(
                            match record
                                .get_str("valueType")
                                .map_err(|_| AppError::Internal)?
                            {
                                "number" => Cell::Number(value.as_f64().ok_or(AppError::Internal)?),
                                "boolean" => {
                                    Cell::Boolean(value.as_bool().ok_or(AppError::Internal)?)
                                }
                                "string" => {
                                    Cell::Text(value.as_str().ok_or(AppError::Internal)?.into())
                                }
                                "json" => Cell::Text(value.to_string()),
                                _ => return Err(AppError::Internal),
                            },
                        )
                    }
                    _ => record
                        .get_str(field)
                        .map(|v| Cell::Text(v.into()))
                        .map_err(|_| AppError::Internal),
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Table {
        headers: fields,
        rows,
    })
}

async fn item_export(
    database: &Database,
    fields: Vec<String>,
    plan: query::QueryPlan,
    limit: usize,
) -> Result<Table, AppError> {
    // Start with the indexed parent key. Each lookup is bounded before unwind,
    // so one dictionary cannot materialize an unbounded BSON array.
    let parent_sort = if plan.parent_sort {
        plan.sort.clone()
    } else {
        doc! {"key":1,"_id":1}
    };
    let row_sort = if plan.parent_sort {
        doc! {"value":1,"_id":1}
    } else {
        plan.sort.clone()
    };
    let mut pipeline = vec![
        doc! {"$match":plan.parent},
        doc! {"$sort":parent_sort},
        doc! {"$lookup":{"from":"dictionary_items","localField":"_id","foreignField":"dictionaryId","pipeline":[{"$match":plan.rows},{"$sort":row_sort},{"$limit":(limit+1) as i64}],"as":"items"}},
        doc! {"$unwind":"$items"},
        doc! {"$project":{"_id":"$items._id","dictionaryKey":"$key","label":"$items.label","value":"$items.value","sort":"$items.sort","enabled":"$items.enabled","createdAt":"$items.createdAt","version":"$items.version"}},
    ];
    if !plan.parent_sort {
        pipeline.push(doc! {"$sort":plan.sort});
    }
    pipeline.push(doc! {"$limit":(limit+1) as i64});
    let records = database
        .collection::<Document>("dictionaries")
        .aggregate(pipeline)
        .max_time(std::time::Duration::from_secs(10))
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    if records.len() > limit {
        return Err(AppError::Validation(
            "导出超过本次行数上限，请缩小字典筛选".into(),
        ));
    }
    let rows = records
        .into_iter()
        .map(|record| {
            fields
                .iter()
                .map(|field| match field.as_str() {
                    "sort" => record
                        .get_i32("sort")
                        .map(|v| Cell::Number(v as f64))
                        .map_err(|_| AppError::Internal),
                    "enabled" => record
                        .get_bool("enabled")
                        .map(Cell::Boolean)
                        .map_err(|_| AppError::Internal),
                    _ => record
                        .get_str(field)
                        .map(|v| Cell::Text(v.into()))
                        .map_err(|_| AppError::Internal),
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Table {
        headers: fields,
        rows,
    })
}
