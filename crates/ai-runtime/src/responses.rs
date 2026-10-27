use crate::{Error, Limits, bounded};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Completed,
    Failed,
    Incomplete,
    Cancelled,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputPart {
    pub output_index: u32,
    pub content_index: u32,
    pub item_id: String,
    pub text: String,
    pub refused: bool,
}
pub struct Reply {
    pub response_id: Option<String>,
    pub status: Status,
    pub refused: bool,
    pub parts: Vec<OutputPart>,
}
impl Reply {
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
pub enum Update {
    Ignored,
    Created {
        sequence: u64,
    },
    Progress {
        sequence: u64,
    },
    Delta {
        sequence: u64,
        output_index: u32,
        content_index: u32,
        text: String,
        refused: bool,
    },
    Terminal {
        sequence: u64,
        status: Status,
    },
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    format_version: u32,
    response_id: Option<String>,
    sequence: Option<u64>,
    last_hash: Option<String>,
    status: Option<Status>,
    parts: Vec<OutputPart>,
    events: u32,
    bytes: usize,
}
pub struct ResponseState {
    limits: Limits,
    state: Snapshot,
}
impl std::fmt::Debug for ResponseState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseState")
            .field("response_present", &self.state.response_id.is_some())
            .field("sequence", &self.state.sequence)
            .field("status", &self.state.status)
            .field("parts", &self.state.parts.len())
            .finish()
    }
}
pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(Error::Protocol)
}
fn index(value: &Value, key: &str) -> Result<u32, Error> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(Error::Protocol)
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let mut result = serde_json::Map::new();
            for key in keys {
                result.insert(key.clone(), canonical(&values[key]));
            }
            Value::Object(result)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}
fn same_key(a: &OutputPart, b: &OutputPart) -> bool {
    a.output_index == b.output_index && a.content_index == b.content_index
}
impl ResponseState {
    pub fn new(limits: Limits) -> Result<Self, Error> {
        Ok(Self {
            limits: limits.validate()?,
            state: Snapshot {
                format_version: 1,
                ..Snapshot::default()
            },
        })
    }
    pub fn sequence(&self) -> Option<u64> {
        self.state.sequence
    }
    pub fn response_id(&self) -> Option<&str> {
        self.state.response_id.as_deref()
    }
    pub fn status(&self) -> Option<Status> {
        self.state.status
    }
    /// Contains private plaintext. Hosts must encrypt it with the run's identity-bound AAD.
    pub fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(&self.state).map_err(|_| Error::Protocol)
    }
    pub fn restore(bytes: &[u8], limits: Limits) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        let state: Snapshot = serde_json::from_slice(bytes).map_err(|_| Error::InvalidJson)?;
        if state.format_version != 1
            || state
                .response_id
                .as_deref()
                .is_some_and(|id| !identifier(id))
            || state.sequence.is_some() != state.last_hash.is_some()
            || state.last_hash.as_deref().is_some_and(|hash| {
                hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            || state.sequence.is_some() && state.events == 0
            || state.response_id.is_none()
                && (!state.parts.is_empty()
                    || state.status.is_some_and(|status| status != Status::Failed))
        {
            return Err(Error::Protocol);
        }
        if state.events > limits.max_events
            || state.bytes > limits.max_stream_bytes
            || state.parts.len() > limits.max_parts
            || state
                .parts
                .iter()
                .map(|part| part.text.len())
                .sum::<usize>()
                > limits.max_text_bytes
        {
            return Err(Error::Limit);
        }
        for (index, part) in state.parts.iter().enumerate() {
            if !identifier(&part.item_id)
                || state.parts[..index].iter().any(|old| same_key(old, part))
            {
                return Err(Error::Protocol);
            }
        }
        Ok(Self { limits, state })
    }
    pub fn apply_json(&mut self, payload: &[u8]) -> Result<Update, Error> {
        let events = self
            .state
            .events
            .checked_add(1)
            .filter(|count| *count <= self.limits.max_events)
            .ok_or(Error::Limit)?;
        let bytes = self
            .state
            .bytes
            .checked_add(payload.len())
            .filter(|size| *size <= self.limits.max_stream_bytes)
            .ok_or(Error::Limit)?;
        if payload.len() > self.limits.max_event_bytes {
            return Err(Error::Limit);
        }
        let mut value: Value = serde_json::from_slice(payload).map_err(|_| Error::InvalidJson)?;
        if !value.is_object() || !bounded(&value) {
            return Err(Error::Limit);
        }
        let kind = field(&value, "type")?.to_owned();
        if let Some(id) = value
            .get("response")
            .and_then(|response| response.get("id"))
        {
            let id = id.as_str().ok_or(Error::Protocol)?;
            if !identifier(id)
                || self
                    .state
                    .response_id
                    .as_deref()
                    .is_some_and(|old| old != id)
            {
                return Err(Error::Protocol);
            }
        }
        if kind == "keepalive" {
            self.state.events = events;
            self.state.bytes = bytes;
            return Ok(Update::Ignored);
        }
        let sequence = value["sequence_number"].as_u64().ok_or(Error::Protocol)?;
        value.as_object_mut().unwrap().remove("obfuscation");
        let hash = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&canonical(&value)).map_err(|_| Error::Protocol)?)
        );
        if self.state.sequence == Some(sequence) && self.state.last_hash.as_deref() == Some(&hash) {
            self.state.events = events;
            self.state.bytes = bytes;
            return Ok(Update::Ignored);
        }
        if self.state.sequence.is_some_and(|old| sequence <= old) {
            return Err(Error::Sequence);
        }
        if self.state.status.is_some() {
            return Err(Error::Protocol);
        }
        let update = match kind.as_str() {
            "response.created" | "response.in_progress" => {
                let response = &value["response"];
                let id = field(response, "id")?;
                let status = field(response, "status")?;
                if !identifier(id)
                    || !matches!(status, "queued" | "in_progress")
                    || self
                        .state
                        .response_id
                        .as_deref()
                        .is_some_and(|old| old != id)
                    || kind == "response.created" && self.state.response_id.is_some()
                {
                    return Err(Error::Protocol);
                }
                self.state.response_id = Some(id.into());
                if kind == "response.created" {
                    Update::Created { sequence }
                } else {
                    Update::Progress { sequence }
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                if self.state.response_id.is_none() {
                    return Err(Error::Protocol);
                }
                let output_index = index(&value, "output_index")?;
                let content_index = index(&value, "content_index")?;
                let item_id = field(&value, "item_id")?;
                let text = field(&value, "delta")?;
                let refused = kind == "response.refusal.delta";
                if !identifier(item_id) {
                    return Err(Error::Protocol);
                }
                let total = self
                    .state
                    .parts
                    .iter()
                    .map(|part| part.text.len())
                    .sum::<usize>();
                if text.len() > self.limits.max_text_bytes.saturating_sub(total) {
                    return Err(Error::Limit);
                }
                let existing = self.state.parts.iter().position(|part| {
                    part.output_index == output_index && part.content_index == content_index
                });
                if let Some(existing) = existing {
                    let part = &mut self.state.parts[existing];
                    if part.item_id != item_id || part.refused != refused {
                        return Err(Error::Protocol);
                    }
                    part.text.push_str(text);
                } else {
                    if self.state.parts.len() >= self.limits.max_parts {
                        return Err(Error::Limit);
                    }
                    self.state.parts.push(OutputPart {
                        output_index,
                        content_index,
                        item_id: item_id.into(),
                        text: text.into(),
                        refused,
                    });
                }
                Update::Delta {
                    sequence,
                    output_index,
                    content_index,
                    text: text.into(),
                    refused,
                }
            }
            "response.output_text.done" | "response.refusal.done" => {
                let output_index = index(&value, "output_index")?;
                let content_index = index(&value, "content_index")?;
                let key = if kind == "response.refusal.done" {
                    "refusal"
                } else {
                    "text"
                };
                let part = self
                    .state
                    .parts
                    .iter()
                    .find(|part| {
                        part.output_index == output_index && part.content_index == content_index
                    })
                    .ok_or(Error::Protocol)?;
                if part.item_id != field(&value, "item_id")?
                    || part.text != field(&value, key)?
                    || part.refused != (kind == "response.refusal.done")
                {
                    return Err(Error::Protocol);
                }
                Update::Progress { sequence }
            }
            "response.completed"
            | "response.failed"
            | "response.incomplete"
            | "response.cancelled" => {
                let response = &value["response"];
                let status = field(response, "status")?;
                if kind != format!("response.{status}") {
                    return Err(Error::Protocol);
                }
                let status = self.accept_response(response)?.ok_or(Error::Protocol)?;
                Update::Terminal { sequence, status }
            }
            "response.output_item.added" | "response.output_item.done" => {
                match field(&value["item"], "type")? {
                    "message" | "reasoning" => {}
                    _ => return Err(Error::UnexpectedOutput),
                }
                Update::Progress { sequence }
            }
            "error" => {
                self.state.status = Some(Status::Failed);
                Update::Terminal {
                    sequence,
                    status: Status::Failed,
                }
            }
            _ if kind.starts_with("response.") && self.state.response_id.is_some() => {
                Update::Progress { sequence }
            }
            _ => return Err(Error::Protocol),
        };
        self.state.sequence = Some(sequence);
        self.state.last_hash = Some(hash);
        self.state.events = events;
        self.state.bytes = bytes;
        Ok(update)
    }
    /// Validates an explicit JSON create/retrieve/cancel response without inventing an event cursor.
    pub fn apply_snapshot(&mut self, payload: &[u8]) -> Result<Option<Status>, Error> {
        let bytes = self
            .state
            .bytes
            .checked_add(payload.len())
            .filter(|size| *size <= self.limits.max_stream_bytes)
            .ok_or(Error::Limit)?;
        let events = self
            .state
            .events
            .checked_add(1)
            .filter(|count| *count <= self.limits.max_events)
            .ok_or(Error::Limit)?;
        let value: Value = serde_json::from_slice(payload).map_err(|_| Error::InvalidJson)?;
        if !bounded(&value) {
            return Err(Error::Limit);
        }
        if field(&value, "object")? != "response" {
            return Err(Error::Protocol);
        }
        let status = self.accept_response(&value)?;
        self.state.bytes = bytes;
        self.state.events = events;
        Ok(status)
    }
    fn accept_response(&mut self, response: &Value) -> Result<Option<Status>, Error> {
        let id = field(response, "id")?;
        if !identifier(id)
            || self
                .state
                .response_id
                .as_deref()
                .is_some_and(|old| old != id)
        {
            return Err(Error::Protocol);
        }
        let status = match field(response, "status")? {
            "queued" | "in_progress" => None,
            "completed" => Some(Status::Completed),
            "failed" => Some(Status::Failed),
            "incomplete" => Some(Status::Incomplete),
            "cancelled" => Some(Status::Cancelled),
            _ => return Err(Error::Protocol),
        };
        if let Some(status) = status {
            let parts = self.snapshot_parts(response, status)?;
            let replaces_parts = !parts.is_empty() || status == Status::Completed;
            if self.state.status.is_some_and(|old| old != status)
                || self.state.status.is_some() && replaces_parts && parts != self.state.parts
            {
                return Err(Error::Protocol);
            }
            if replaces_parts {
                if self.state.parts.iter().any(|old| {
                    parts
                        .iter()
                        .find(|new| same_key(old, new))
                        .is_none_or(|new| {
                            old.item_id != new.item_id
                                || old.refused != new.refused
                                || !new.text.starts_with(&old.text)
                        })
                }) {
                    return Err(Error::Protocol);
                }
                self.state.parts = parts;
            }
        } else if self.state.status.is_some() {
            return Err(Error::Protocol);
        }
        self.state.response_id = Some(id.into());
        self.state.status = status;
        Ok(status)
    }
    fn snapshot_parts(&self, response: &Value, status: Status) -> Result<Vec<OutputPart>, Error> {
        let Some(output) = response.get("output") else {
            return if status == Status::Completed {
                Err(Error::Protocol)
            } else {
                Ok(Vec::new())
            };
        };
        let output = output.as_array().ok_or(Error::Protocol)?;
        if output.len() > 128 {
            return Err(Error::Limit);
        }
        let mut parts = Vec::new();
        let mut bytes = 0usize;
        for (output_index, item) in output.iter().enumerate() {
            match field(item, "type")? {
                "reasoning" => continue,
                "message" => {}
                _ => return Err(Error::UnexpectedOutput),
            }
            if field(item, "role")? != "assistant" {
                return Err(Error::Protocol);
            }
            let id = field(item, "id")?;
            if !identifier(id) {
                return Err(Error::Protocol);
            }
            let content = item["content"].as_array().ok_or(Error::Protocol)?;
            for (content_index, part) in content.iter().enumerate() {
                let kind = field(part, "type")?;
                let (refused, key) = match kind {
                    "output_text" => (false, "text"),
                    "refusal" => (true, "refusal"),
                    _ => return Err(Error::UnexpectedOutput),
                };
                let text = field(part, key)?;
                bytes = bytes
                    .checked_add(text.len())
                    .filter(|bytes| *bytes <= self.limits.max_text_bytes)
                    .ok_or(Error::Limit)?;
                if parts.len() >= self.limits.max_parts {
                    return Err(Error::Limit);
                }
                parts.push(OutputPart {
                    output_index: output_index as u32,
                    content_index: u32::try_from(content_index).map_err(|_| Error::Limit)?,
                    item_id: id.into(),
                    text: text.into(),
                    refused,
                });
            }
        }
        Ok(parts)
    }
    pub fn finish(mut self) -> Result<Reply, Error> {
        let status = self.state.status.ok_or(Error::Truncated)?;
        self.state
            .parts
            .sort_by_key(|part| (part.output_index, part.content_index));
        Ok(Reply {
            response_id: self.state.response_id,
            status,
            refused: self.state.parts.iter().any(|part| part.refused),
            parts: self.state.parts,
        })
    }
}
