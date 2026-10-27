use crate::{Error, key, label};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    String,
    Integer,
    Number,
    Boolean,
    DateTime,
    ObjectId,
    Json,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Module {
    pub key: String,
    pub label: String,
    pub description: String,
    pub fields: Vec<Field>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Analysis {
    pub summary: String,
    pub modules: Vec<Module>,
    pub risks: Vec<String>,
    pub questions: Vec<String>,
}
impl Analysis {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > 65536 {
            return Err(Error::Limit);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| Error::Analysis)?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.summary.trim().is_empty()
            || self.summary.len() > 4096
            || self.modules.len() > 16
            || self.risks.len() > 32
            || self.questions.len() > 32
        {
            return Err(Error::Analysis);
        }
        let mut modules = HashSet::new();
        let mut count = 0;
        for module in &self.modules {
            count += module.fields.len();
            if !key(&module.key, 48)
                || !modules.insert(&module.key)
                || !label(&module.label, 80)
                || module.description.len() > 2048
                || module.fields.len() > 32
                || count > 256
            {
                return Err(Error::Analysis);
            }
            let mut fields = HashSet::new();
            for field in &module.fields {
                if field.name.is_empty()
                    || field.name.len() > 64
                    || !field
                        .name
                        .bytes()
                        .next()
                        .is_some_and(|byte| byte.is_ascii_alphabetic())
                    || !field
                        .name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    || !fields.insert(&field.name)
                    || !label(&field.label, 80)
                {
                    return Err(Error::Analysis);
                }
            }
        }
        if self
            .risks
            .iter()
            .chain(&self.questions)
            .any(|value| value.trim().is_empty() || value.len() > 2048)
        {
            return Err(Error::Analysis);
        }
        Ok(())
    }
    pub fn schema() -> Value {
        let field = json!({"type":"object","properties":{"name":{"type":"string"},"label":{"type":"string"},"kind":{"type":"string","enum":["string","integer","number","boolean","dateTime","objectId","json"]},"required":{"type":"boolean"}},"required":["name","label","kind","required"],"additionalProperties":false});
        let module = json!({"type":"object","properties":{"key":{"type":"string"},"label":{"type":"string"},"description":{"type":"string"},"fields":{"type":"array","items":field}},"required":["key","label","description","fields"],"additionalProperties":false});
        json!({"type":"object","properties":{"summary":{"type":"string"},"modules":{"type":"array","items":module},"risks":{"type":"array","items":{"type":"string"}},"questions":{"type":"array","items":{"type":"string"}}},"required":["summary","modules","risks","questions"],"additionalProperties":false})
    }
}
