//! Compiles package aliases against the actual, explicit business handler contracts.
use crate::{common::error::AppError, modules::mcp::business};
use jsonschema::{PatternOptions, Validator};
use rmcp::model::Tool;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use tri_extension_package::package::ToolBinding;

pub struct CompiledTool {
    operation: String,
    permission: &'static str,
    read_only: bool,
    source: Validator,
    native: Validator,
    mappings: BTreeMap<String, String>,
    outer: BTreeSet<String>,
    wrapper: Option<&'static str>,
    description: Tool,
}
fn invalid() -> AppError {
    AppError::Validation("插件工具 Schema、目标操作或字段映射无效".into())
}
pub(crate) fn bounded(value: &Value, max: usize) -> bool {
    let mut stack = vec![(value, 0usize)];
    let mut nodes = 0;
    while let Some((value, depth)) = stack.pop() {
        nodes += 1;
        if depth > 32 || nodes > 4096 {
            return false;
        }
        match value {
            Value::Array(values) => stack.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => stack.extend(values.values().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    serde_json::to_vec(value).is_ok_and(|bytes| bytes.len() <= max)
}
fn validator(schema: &Value) -> Result<Validator, AppError> {
    if !bounded(schema, 64 * 1024) {
        return Err(invalid());
    }
    jsonschema::options()
        .offline()
        .with_pattern_options(
            PatternOptions::regex()
                .size_limit(1024 * 1024)
                .dfa_size_limit(1024 * 1024),
        )
        .should_validate_formats(true)
        .should_ignore_unknown_formats(false)
        .build(schema)
        .map_err(|_| invalid())
}
fn names(value: Option<&Value>) -> Result<BTreeSet<String>, AppError> {
    match value {
        None => Ok(BTreeSet::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|v| v.as_str().map(String::from).ok_or_else(invalid))
            .collect(),
        _ => Err(invalid()),
    }
}
pub fn compile(binding: &ToolBinding) -> Result<CompiledTool, AppError> {
    let contract = business::contracts()
        .into_iter()
        .find(|row| row.tool.name == binding.operation)
        .ok_or_else(invalid)?;
    if binding.input_schema["type"] != "object" {
        return Err(invalid());
    }
    let source_properties = binding.input_schema["properties"]
        .as_object()
        .ok_or_else(invalid)?;
    if source_properties.len() > 64 || binding.mappings.len() > 64 {
        return Err(invalid());
    }
    let native_schema = Value::Object(contract.tool.input_schema.as_ref().clone());
    let properties = native_schema["properties"]
        .as_object()
        .ok_or_else(invalid)?;
    let wrapper = ["input", "query"].into_iter().find(|key| {
        properties
            .get(*key)
            .is_some_and(|value| value["type"] == "object")
    });
    let outer = properties
        .keys()
        .filter(|key| Some(key.as_str()) != wrapper)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut targets = outer.clone();
    let mut required = names(native_schema.get("required"))?;
    if let Some(wrapper) = wrapper {
        let nested = properties[wrapper]["properties"]
            .as_object()
            .ok_or_else(invalid)?;
        targets.extend(nested.keys().cloned());
        required.remove(wrapper);
        required.extend(names(properties[wrapper].get("required"))?);
    }
    let declared_required = names(binding.input_schema.get("required"))?;
    if binding.mappings.iter().any(|(target, source)| {
        !targets.contains(target) || !source_properties.contains_key(source)
    }) || required.iter().any(|target| {
        binding
            .mappings
            .get(target)
            .is_none_or(|source| !declared_required.contains(source))
    }) {
        return Err(invalid());
    }
    let mut description = contract.tool;
    description.description = Some(binding.description.clone().into());
    description.input_schema =
        std::sync::Arc::new(binding.input_schema.as_object().unwrap().clone());
    Ok(CompiledTool {
        operation: binding.operation.clone(),
        permission: contract.permission,
        read_only: contract.read_only,
        source: validator(&binding.input_schema)?,
        native: validator(&native_schema)?,
        mappings: binding.mappings.clone(),
        outer,
        wrapper,
        description,
    })
}
impl CompiledTool {
    pub fn permission(&self) -> &'static str {
        self.permission
    }
    pub fn read_only(&self) -> bool {
        self.read_only
    }
    pub fn operation(&self) -> &str {
        &self.operation
    }
    pub fn tool(&self, name: String) -> Tool {
        let mut tool = self.description.clone();
        tool.name = name.into();
        tool
    }
    pub fn arguments(&self, value: &Value) -> Result<Value, AppError> {
        if !value.is_object() || !bounded(value, 64 * 1024) || !self.source.is_valid(value) {
            return Err(invalid());
        }
        let mut result = Map::new();
        let mut nested = Map::new();
        for (target, source) in &self.mappings {
            if let Some(value) = value.get(source) {
                if self.outer.contains(target) {
                    result.insert(target.clone(), value.clone());
                } else {
                    nested.insert(target.clone(), value.clone());
                }
            }
        }
        if let Some(wrapper) = self.wrapper {
            result.insert(wrapper.into(), Value::Object(nested));
        }
        let result = Value::Object(result);
        if !bounded(&result, 64 * 1024) || !self.native.is_valid(&result) {
            return Err(invalid());
        }
        Ok(result)
    }
}
