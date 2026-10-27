use crate::{Error, label};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
const LIMIT: usize = 4 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}
impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub title: String,
    pub outcome: Outcome,
    pub prompt: String,
    pub text: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Transcript {
    pub title: String,
    pub outcome: Outcome,
    pub entries: Vec<Entry>,
}
pub struct Artifact {
    pub bytes: Vec<u8>,
    pub size: usize,
    pub sha256: String,
}
struct Writer(String);
impl Writer {
    fn push(&mut self, value: &str) -> Result<(), Error> {
        if self
            .0
            .len()
            .checked_add(value.len())
            .is_none_or(|size| size > LIMIT)
        {
            return Err(Error::Limit);
        }
        self.0.push_str(value);
        Ok(())
    }
    fn heading(&mut self, value: &str) -> Result<(), Error> {
        for c in value.chars() {
            if c.is_ascii_punctuation() {
                self.push("\\")?;
            }
            self.push(c.encode_utf8(&mut [0; 4]))?;
        }
        Ok(())
    }
    fn fenced(&mut self, value: &str) -> Result<(), Error> {
        let mut maximum = 0usize;
        let mut current = 0usize;
        for byte in value.bytes() {
            if byte == b'`' {
                current += 1;
                maximum = maximum.max(current);
            } else {
                current = 0;
            }
        }
        let fence = "`".repeat(maximum.saturating_add(1).max(3));
        self.push(&fence)?;
        self.push("text\n")?;
        self.push(value)?;
        self.push("\n")?;
        self.push(&fence)?;
        self.push("\n\n")
    }
}
impl Transcript {
    pub fn markdown(&self) -> Result<Artifact, Error> {
        if !label(&self.title, 120) || self.entries.len() > 16 {
            return Err(Error::Input);
        }
        let mut output = Writer(String::new());
        output.push("# ")?;
        output.heading(&self.title)?;
        output.push("\n\n状态：")?;
        output.push(self.outcome.as_str())?;
        output.push("\n\n")?;
        for entry in &self.entries {
            if !label(&entry.title, 80)
                || entry.prompt.len() > 65536
                || entry.text.len() > 1024 * 1024
            {
                return Err(Error::Limit);
            }
            output.push("## ")?;
            output.heading(&entry.title)?;
            output.push("\n\n状态：")?;
            output.push(entry.outcome.as_str())?;
            output.push("\n\n### 提示\n\n")?;
            output.fenced(&entry.prompt)?;
            output.push("### 输出\n\n")?;
            output.fenced(&entry.text)?;
        }
        let bytes = output.0.into_bytes();
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        Ok(Artifact {
            size: bytes.len(),
            bytes,
            sha256,
        })
    }
}
