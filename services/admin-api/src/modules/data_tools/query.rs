use crate::common::error::AppError;
use mongodb::bson::{Bson, DateTime, Document, doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FilterOp {
    Eq,
    Prefix,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataFilter {
    pub field: String,
    pub op: FilterOp,
    pub value: Value,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Asc,
    Desc,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataSort {
    pub field: String,
    pub direction: Direction,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Boolean,
    Integer,
    Timestamp,
    Id,
    Choice,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryField {
    pub field: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub operators: &'static [FilterOp],
    pub choices: &'static [&'static str],
}
const TEXT: &[FilterOp] = &[FilterOp::Eq, FilterOp::Prefix, FilterOp::In];
const RANGE: &[FilterOp] = &[
    FilterOp::Eq,
    FilterOp::Gt,
    FilterOp::Gte,
    FilterOp::Lt,
    FilterOp::Lte,
    FilterOp::In,
];
const EQUAL: &[FilterOp] = &[FilterOp::Eq];
const CHOICE: &[FilterOp] = &[FilterOp::Eq, FilterOp::In];
const fn text(field: &'static str, label: &'static str) -> QueryField {
    QueryField {
        field,
        label,
        kind: FieldKind::Text,
        operators: TEXT,
        choices: &[],
    }
}
const fn integer(field: &'static str, label: &'static str) -> QueryField {
    QueryField {
        field,
        label,
        kind: FieldKind::Integer,
        operators: RANGE,
        choices: &[],
    }
}
const fn date() -> QueryField {
    QueryField {
        field: "createdAt",
        label: "创建时间",
        kind: FieldKind::Timestamp,
        operators: RANGE,
        choices: &[],
    }
}
const fn enabled() -> QueryField {
    QueryField {
        field: "enabled",
        label: "启用",
        kind: FieldKind::Boolean,
        operators: EQUAL,
        choices: &[],
    }
}
const PARAMS: &[QueryField] = &[
    text("key", "标识"),
    text("name", "名称"),
    text("group", "分组"),
    QueryField {
        field: "valueType",
        label: "值类型",
        kind: FieldKind::Choice,
        operators: CHOICE,
        choices: &["string", "number", "boolean", "json"],
    },
    date(),
    integer("version", "版本"),
];
const USERS: &[QueryField] = &[
    QueryField {
        field: "id",
        label: "ID",
        kind: FieldKind::Id,
        operators: CHOICE,
        choices: &[],
    },
    text("username", "账号"),
    text("displayName", "显示名"),
    QueryField {
        field: "status",
        label: "状态",
        kind: FieldKind::Choice,
        operators: CHOICE,
        choices: &["active", "disabled"],
    },
    date(),
    integer("version", "版本"),
];
const DICTS: &[QueryField] = &[
    text("key", "标识"),
    text("name", "名称"),
    enabled(),
    date(),
    integer("version", "版本"),
];
const ITEMS: &[QueryField] = &[
    text("dictionaryKey", "父字典标识"),
    text("label", "明细名称"),
    text("value", "明细值"),
    integer("sort", "排序"),
    enabled(),
    date(),
    integer("version", "版本"),
];
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuerySchema {
    pub fields: &'static [QueryField],
    pub sort_fields: &'static [&'static str],
}
pub fn schema(resource: &str) -> Result<QuerySchema, AppError> {
    Ok(match resource {
        "parameters" => QuerySchema {
            fields: PARAMS,
            sort_fields: &["key", "name", "group", "createdAt"],
        },
        "users" => QuerySchema {
            fields: USERS,
            sort_fields: &["username", "displayName", "createdAt", "version"],
        },
        "dictionaries" => QuerySchema {
            fields: DICTS,
            sort_fields: &["key", "name", "createdAt"],
        },
        "dictionaryItems" => QuerySchema {
            fields: ITEMS,
            sort_fields: &["dictionaryKey", "value", "label", "sort", "createdAt"],
        },
        _ => return Err(invalid()),
    })
}
fn invalid() -> AppError {
    AppError::Validation("筛选字段、操作或值类型无效".into())
}
pub fn prefix(value: &str) -> String {
    let mut output = String::from("^");
    for c in value.chars() {
        if ".+*?^${}()|[]\\".contains(c) {
            output.push('\\');
        }
        output.push(c);
    }
    output
}
fn scalar(resource: &str, field: &QueryField, value: &Value) -> Result<Bson, AppError> {
    match field.kind {
        FieldKind::Text => {
            let text = value
                .as_str()
                .filter(|text| text.chars().count() <= 255)
                .ok_or_else(invalid)?;
            Ok(Bson::String(
                if resource == "users" && field.field == "username" {
                    text.to_lowercase()
                } else {
                    text.to_owned()
                },
            ))
        }
        FieldKind::Choice => {
            let text = value
                .as_str()
                .filter(|text| field.choices.contains(text))
                .ok_or_else(invalid)?;
            Ok(Bson::String(text.into()))
        }
        FieldKind::Boolean => value.as_bool().map(Bson::Boolean).ok_or_else(invalid),
        FieldKind::Integer => {
            let number = value
                .as_f64()
                .filter(|v| v.is_finite() && v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0)
                .ok_or_else(invalid)?;
            if field.field == "sort" && (number < i32::MIN as f64 || number > i32::MAX as f64) {
                return Err(invalid());
            }
            Ok(Bson::Int64(number as i64))
        }
        FieldKind::Id => ObjectId::parse_str(value.as_str().ok_or_else(invalid)?)
            .map(Bson::ObjectId)
            .map_err(|_| invalid()),
        FieldKind::Timestamp => {
            let date = chrono::DateTime::parse_from_rfc3339(value.as_str().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
            Ok(Bson::DateTime(DateTime::from_millis(
                date.timestamp_millis(),
            )))
        }
    }
}
pub struct QueryPlan {
    pub parent: Document,
    pub rows: Document,
    pub sort: Document,
    pub parent_sort: bool,
}
pub fn plan(
    resource: &str,
    search: &str,
    filters: &[DataFilter],
    sort: Option<&DataSort>,
) -> Result<QueryPlan, AppError> {
    let schema = schema(resource)?;
    if search.chars().count() > 100 || search.contains('\0') || filters.len() > 8 {
        return Err(invalid());
    }
    let mut parent = vec![doc! {"deletedAt":null}];
    let mut rows = vec![doc! {"deletedAt":null}];
    let search = if resource == "users" {
        search.trim().to_lowercase()
    } else {
        search.trim().to_owned()
    };
    if !search.is_empty() {
        let field = if resource == "users" {
            "usernameKey"
        } else {
            "key"
        };
        let mut item = doc! {};
        item.insert(field, doc! {"$regex":prefix(&search)});
        if resource == "dictionaryItems" {
            parent.push(item)
        } else {
            rows.push(item)
        }
    }
    for filter in filters {
        let field = schema
            .fields
            .iter()
            .find(|field| field.field == filter.field)
            .ok_or_else(invalid)?;
        if !field.operators.contains(&filter.op) {
            return Err(invalid());
        }
        let value = if filter.op == FilterOp::In {
            let values = filter
                .value
                .as_array()
                .filter(|v| !v.is_empty() && v.len() <= 32)
                .ok_or_else(invalid)?;
            Bson::Array(
                values
                    .iter()
                    .map(|value| scalar(resource, field, value))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        } else {
            scalar(resource, field, &filter.value)?
        };
        let predicate = match filter.op {
            FilterOp::Eq => value,
            FilterOp::Prefix => {
                if value.as_str().is_some_and(|value| value.contains('\0')) {
                    return Err(invalid());
                }
                Bson::Document(doc! {"$regex":prefix(value.as_str().ok_or_else(invalid)?)})
            }
            operation => {
                let op = match operation {
                    FilterOp::Gt => "$gt",
                    FilterOp::Gte => "$gte",
                    FilterOp::Lt => "$lt",
                    FilterOp::Lte => "$lte",
                    FilterOp::In => "$in",
                    _ => return Err(invalid()),
                };
                let mut condition = doc! {};
                condition.insert(op, value);
                Bson::Document(condition)
            }
        };
        let name = if filter.field == "id" {
            "_id"
        } else if resource == "users" && filter.field == "username" {
            "usernameKey"
        } else if filter.field == "dictionaryKey" {
            "key"
        } else {
            filter.field.as_str()
        };
        let mut item = doc! {};
        item.insert(name, predicate);
        if resource == "dictionaryItems" && filter.field == "dictionaryKey" {
            parent.push(item)
        } else {
            rows.push(item)
        }
    }
    let default = if resource == "users" {
        "username"
    } else if resource == "dictionaryItems" {
        "dictionaryKey"
    } else {
        "key"
    };
    let field = sort.map(|v| v.field.as_str()).unwrap_or(default);
    if !schema.sort_fields.contains(&field) {
        return Err(invalid());
    }
    let direction = if matches!(sort.map(|v| v.direction), Some(Direction::Desc)) {
        -1
    } else {
        1
    };
    let parent_sort = resource == "dictionaryItems" && field == "dictionaryKey";
    let name = if resource == "users" && field == "username" {
        "usernameKey"
    } else if parent_sort {
        "key"
    } else {
        field
    };
    let mut sorting = doc! {};
    sorting.insert(name, direction);
    sorting.insert("_id", direction);
    Ok(QueryPlan {
        parent: doc! {"$and":parent},
        rows: doc! {"$and":rows},
        sort: sorting,
        parent_sort,
    })
}
