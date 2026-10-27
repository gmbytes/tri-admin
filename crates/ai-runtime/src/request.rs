use crate::{Error, responses::identifier};
use std::time::Duration;

// Deliberately no Debug: these strings contain private user input.
pub struct CreateRequest {
    pub model: String,
    pub input: String,
    pub instructions: Option<String>,
    pub max_output_tokens: u32,
    pub background: bool,
    pub stream: bool,
    pub format: Option<JsonFormat>,
}
// The trusted host supplies this schema; HTTP callers never pass it through.
pub struct JsonFormat {
    pub name: String,
    pub schema: serde_json::Value,
}
impl JsonFormat {
    fn validate(&self) -> Result<(), Failure> {
        if self.name.is_empty()
            || self.name.len() > 64
            || !self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            || !self.schema.is_object()
            || !crate::bounded(&self.schema)
        {
            return Err(Failure::input());
        }
        if serde_json::to_vec(&self.schema)
            .map_err(|_| Failure::input())?
            .len()
            > 65536
        {
            return Err(Failure::input());
        }
        let mut nodes = vec![&self.schema];
        while let Some(value) = nodes.pop() {
            match value {
                serde_json::Value::Object(values) => {
                    for (key, value) in values {
                        if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef")
                            && !value.as_str().is_some_and(|value| value.starts_with('#'))
                        {
                            return Err(Failure::input());
                        }
                        nodes.push(value);
                    }
                }
                serde_json::Value::Array(values) => nodes.extend(values),
                _ => {}
            }
        }
        Ok(())
    }
}
impl CreateRequest {
    pub(crate) fn body(&self) -> Result<Vec<u8>, Failure> {
        if self.model.is_empty()
            || self.model.len() > 160
            || !self
                .model
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
            || self.input.trim().is_empty()
            || self.input.len() > 65536
            || self
                .instructions
                .as_ref()
                .is_some_and(|text| text.len() > 16384)
            || !(1..=32768).contains(&self.max_output_tokens)
        {
            return Err(Failure::input());
        }
        let mut value = serde_json::json!({"model":self.model,"input":self.input,"max_output_tokens":self.max_output_tokens,"background":self.background,"stream":self.stream,"store":self.background,"tools":[]});
        if let Some(instructions) = &self.instructions {
            value["instructions"] = serde_json::Value::String(instructions.clone());
        }
        if let Some(format) = &self.format {
            format.validate()?;
            value["text"] = serde_json::json!({"format":{"type":"json_schema","name":format.name,"schema":format.schema,"strict":true}});
        }
        serde_json::to_vec(&value).map_err(|_| Failure::input())
    }
}
/// Metadata restored by the trusted host from the original accepted request.
/// It is not evidence of authorization: the host rechecks ownership and binding.
#[derive(Clone)]
pub struct Reference {
    pub(crate) id: String,
    pub(crate) background: bool,
    pub(crate) streamed: bool,
}
impl Reference {
    pub fn new(id: &str, background: bool, streamed: bool) -> Result<Self, Failure> {
        if !identifier(id) {
            return Err(Failure::input());
        }
        Ok(Self {
            id: id.into(),
            background,
            streamed,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub total_timeout: Duration,
    pub idle_timeout: Duration,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            total_timeout: Duration::from_secs(120),
            idle_timeout: Duration::from_secs(30),
        }
    }
}
impl Policy {
    pub(crate) fn validate(self) -> Result<Self, Failure> {
        if self.total_timeout.is_zero()
            || self.total_timeout > Duration::from_secs(120)
            || self.idle_timeout.is_zero()
            || self.idle_timeout > Duration::from_secs(30)
            || self.idle_timeout > self.total_timeout
        {
            return Err(Failure::input());
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Input,
    Cancelled,
    Timeout,
    Transport,
    Status(u16),
    Protocol(Error),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("AI HTTP operation failed: {kind:?}; submission possible: {may_have_been_sent}")]
pub struct Failure {
    pub kind: FailureKind,
    pub may_have_been_sent: bool,
}
impl Failure {
    pub(crate) fn input() -> Self {
        Self {
            kind: FailureKind::Input,
            may_have_been_sent: false,
        }
    }
    pub(crate) fn sent(kind: FailureKind) -> Self {
        Self {
            kind,
            may_have_been_sent: true,
        }
    }
    pub(crate) fn reading(error: Error) -> Self {
        Self::sent(match error {
            Error::Cancelled => FailureKind::Cancelled,
            Error::Timeout => FailureKind::Timeout,
            Error::Transport => FailureKind::Transport,
            other => FailureKind::Protocol(other),
        })
    }
}
