//! Bounded AI upstream protocols. The host owns credentials, authorization and storage.
pub mod http;
mod request;
mod responses;
mod sse;
pub use responses::{OutputPart, Reply, ResponseState, Status, Update};
pub use sse::{Frame, frames};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_event_bytes: usize,
    pub max_stream_bytes: usize,
    pub max_events: u32,
    pub max_parts: usize,
    pub max_text_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_event_bytes: 64 * 1024,
            max_stream_bytes: 4 * 1024 * 1024,
            max_events: 4096,
            max_parts: 32,
            max_text_bytes: 1024 * 1024,
        }
    }
}
impl Limits {
    fn validate(self) -> Result<Self, Error> {
        let max = Self::default();
        if self.max_event_bytes == 0
            || self.max_event_bytes > max.max_event_bytes
            || self.max_stream_bytes == 0
            || self.max_stream_bytes > max.max_stream_bytes
            || self.max_events == 0
            || self.max_events > max.max_events
            || self.max_parts == 0
            || self.max_parts > max.max_parts
            || self.max_text_bytes == 0
            || self.max_text_bytes > max.max_text_bytes
        {
            return Err(Error::Limit);
        }
        Ok(self)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("AI protocol limit exceeded")]
    Limit,
    #[error("AI response JSON is invalid")]
    InvalidJson,
    #[error("AI response protocol is invalid")]
    Protocol,
    #[error("AI event sequence is inconsistent")]
    Sequence,
    #[error("AI output contains an unsupported operation")]
    UnexpectedOutput,
    #[error("AI upstream transport failed")]
    Transport,
    #[error("AI upstream deadline exceeded")]
    Timeout,
    #[error("AI upstream read was cancelled")]
    Cancelled,
    #[error("AI stream ended without a validated terminal response")]
    Truncated,
}

fn bounded(value: &serde_json::Value) -> bool {
    let mut stack = vec![(value, 0usize)];
    let mut nodes = 0;
    while let Some((value, depth)) = stack.pop() {
        nodes += 1;
        if nodes > 4096 || depth > 32 {
            return false;
        }
        match value {
            serde_json::Value::Array(values) => stack.extend(values.iter().map(|v| (v, depth + 1))),
            serde_json::Value::Object(values) => {
                stack.extend(values.values().map(|v| (v, depth + 1)))
            }
            _ => {}
        }
    }
    true
}
