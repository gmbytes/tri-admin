use mongodb::bson::{Bson, DateTime, Document};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const SAFE_INTEGER: i64 = 9_007_199_254_740_991;
#[derive(Clone, Debug, thiserror::Error)]
#[error("{0}")]
pub struct SchemaError(pub &'static str);
type Result<T> = std::result::Result<T, SchemaError>;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Text,
    Integer,
    Number,
    Boolean,
    Datetime,
    TextList,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub kind: Kind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub searchable: bool,
    #[serde(default)]
    pub unique: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Schema {
    pub resource: String,
    pub label: String,
    pub fields: Vec<Field>,
}
fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 80 && !value.chars().any(char::is_control)
}
fn resource(value: &str) -> bool {
    (3..=48).contains(&value.len())
        && value.bytes().next().is_some_and(|v| v.is_ascii_lowercase())
        && value
            .bytes()
            .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'_')
        && ![
            "users",
            "roles",
            "menus",
            "sessions",
            "admins",
            "applications",
            "api_tokens",
            "schema",
            "system",
        ]
        .contains(&value)
}
fn field_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value.bytes().next().is_some_and(|v| v.is_ascii_lowercase())
        && value.bytes().all(|v| v.is_ascii_alphanumeric())
        && ![
            "id",
            "ownerid",
            "version",
            "createdat",
            "updatedat",
            "deletedat",
            "application",
            "password",
            "passwordhash",
            "secret",
            "token",
            "authorization",
            "constructor",
            "prototype",
        ]
        .contains(&value.to_ascii_lowercase().as_str())
}
impl Schema {
    pub fn parse(value: Value) -> Result<Self> {
        let schema: Self =
            serde_json::from_value(value).map_err(|_| SchemaError("资源描述字段无效"))?;
        schema.validate()?;
        Ok(schema)
    }
    pub fn validate(&self) -> Result<()> {
        if !resource(&self.resource)
            || !label(&self.label)
            || !(1..=40).contains(&self.fields.len())
        {
            return Err(SchemaError("资源标识、名称或字段数量无效"));
        }
        let mut names = HashSet::new();
        for field in &self.fields {
            if !field_name(&field.name)
                || !label(&field.label)
                || !names.insert(field.name.to_ascii_lowercase())
                || field.searchable && field.kind != Kind::Text
                || field.unique && (field.kind != Kind::Text || !field.required)
            {
                return Err(SchemaError("字段标识、名称、类型或索引策略无效"));
            }
        }
        if self.fields.iter().filter(|v| v.searchable).count() > 3
            || self.fields.iter().filter(|v| v.unique).count() > 5
        {
            return Err(SchemaError("搜索或唯一字段超过上限"));
        }
        Ok(())
    }
    pub fn collection(&self) -> String {
        format!("records_{}", self.resource)
    }
    pub fn checksum(&self) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|_| SchemaError("资源描述不可编码"))?)
        ))
    }
    pub fn values(&self, value: &Value) -> Result<Document> {
        let value = value.as_object().ok_or(SchemaError("values 必须为对象"))?;
        if value
            .keys()
            .any(|key| !self.fields.iter().any(|field| field.name == *key))
        {
            return Err(SchemaError("values 包含未定义字段"));
        }
        let mut result = Document::new();
        for field in &self.fields {
            let value = value.get(&field.name).unwrap_or(&Value::Null);
            let stored = if value.is_null() {
                if field.required {
                    return Err(SchemaError("必填字段不能为空"));
                }
                Bson::Null
            } else {
                encode(field, value)?
            };
            result.insert(&field.name, stored);
        }
        self.public_values(&result)?;
        Ok(result)
    }
    pub fn public_values(&self, document: &Document) -> Result<Value> {
        if document
            .keys()
            .any(|key| !self.fields.iter().any(|field| field.name == *key))
        {
            return Err(SchemaError("存储字段与资源描述不一致"));
        }
        let mut output = Map::new();
        for field in &self.fields {
            let stored = document.get(&field.name).unwrap_or(&Bson::Null);
            if !matches!(
                (field.kind, stored),
                (_, Bson::Null)
                    | (Kind::Text, Bson::String(_))
                    | (Kind::Integer, Bson::Int64(_))
                    | (Kind::Number, Bson::Double(_))
                    | (Kind::Boolean, Bson::Boolean(_))
                    | (Kind::Datetime, Bson::DateTime(_))
                    | (Kind::TextList, Bson::Array(_))
            ) {
                return Err(SchemaError("存储字段类型与资源描述不一致"));
            }
            let value = match stored {
                Bson::Null => Value::Null,
                Bson::String(value) => Value::String(value.clone()),
                Bson::Int64(value) => Value::from(*value),
                Bson::Double(value) => serde_json::Number::from_f64(*value)
                    .map(Value::Number)
                    .ok_or(SchemaError("存储数字无效"))?,
                Bson::Boolean(value) => Value::Bool(*value),
                Bson::DateTime(value) => Value::String(
                    chrono::DateTime::from_timestamp_millis(value.timestamp_millis())
                        .ok_or(SchemaError("存储时间无效"))?
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                ),
                Bson::Array(values) => Value::Array(
                    values
                        .iter()
                        .map(|v| match v {
                            Bson::String(v) => Ok(Value::String(v.clone())),
                            _ => Err(SchemaError("存储列表无效")),
                        })
                        .collect::<Result<_>>()?,
                ),
                _ => return Err(SchemaError("存储字段类型无效")),
            };
            if value.is_null() {
                if field.required {
                    return Err(SchemaError("存储必填字段缺失"));
                }
            } else {
                encode(field, &value)?;
            }
            output.insert(field.name.clone(), value);
        }
        let value = Value::Object(output);
        if serde_json::to_vec(&value)
            .map_err(|_| SchemaError("记录不可编码"))?
            .len()
            > 64 * 1024
        {
            return Err(SchemaError("记录内容超过 64 KiB"));
        }
        Ok(value)
    }
}
fn encode(field: &Field, value: &Value) -> Result<Bson> {
    let invalid = SchemaError("字段类型或值范围无效");
    Ok(match field.kind {
        Kind::Text => {
            let text = value
                .as_str()
                .filter(|v| v.chars().count() <= 4096 && (!field.required || !v.trim().is_empty()))
                .ok_or(invalid)?;
            Bson::String(text.into())
        }
        Kind::Integer => Bson::Int64(
            value
                .as_i64()
                .filter(|v| (-SAFE_INTEGER..=SAFE_INTEGER).contains(v))
                .ok_or(invalid)?,
        ),
        Kind::Number => Bson::Double(
            value
                .as_f64()
                .filter(|v| v.is_finite() && v.abs() <= SAFE_INTEGER as f64)
                .ok_or(invalid)?,
        ),
        Kind::Boolean => Bson::Boolean(value.as_bool().ok_or(invalid)?),
        Kind::Datetime => {
            let parsed =
                chrono::DateTime::parse_from_rfc3339(value.as_str().ok_or(invalid.clone())?)
                    .map_err(|_| invalid.clone())?;
            if !(1..=9999).contains(&chrono::Datelike::year(&parsed)) {
                return Err(invalid);
            }
            Bson::DateTime(DateTime::from_millis(parsed.timestamp_millis()))
        }
        Kind::TextList => {
            let list = value
                .as_array()
                .filter(|v| v.len() <= 100)
                .ok_or(invalid.clone())?;
            Bson::Array(
                list.iter()
                    .map(|v| {
                        v.as_str()
                            .filter(|v| v.chars().count() <= 256)
                            .map(|v| Bson::String(v.into()))
                            .ok_or(invalid.clone())
                    })
                    .collect::<Result<_>>()?,
            )
        }
    })
}
