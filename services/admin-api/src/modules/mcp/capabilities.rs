use crate::config::ConfigError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tri_mcp_runtime::process::{Limits, ProgramSpec};
#[derive(Default)]
pub struct Catalog {
    programs: BTreeMap<String, Capability>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capability {
    pub id: String,
    pub label: String,
    program: PathBuf,
    #[serde(default)]
    arguments: Vec<String>,
    working_directory: PathBuf,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    pub allowed_tools: Vec<String>,
    #[serde(default = "startup")]
    startup_timeout_ms: u64,
    #[serde(default = "request")]
    request_timeout_ms: u64,
    #[serde(default = "stop")]
    stop_timeout_ms: u64,
}
fn startup() -> u64 {
    10_000
}
fn request() -> u64 {
    30_000
}
fn stop() -> u64 {
    5_000
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct File {
    format_version: u32,
    programs: Vec<Capability>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityView {
    pub id: String,
    pub label: String,
    pub allowed_tools: Vec<String>,
}
pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}
impl Catalog {
    pub fn load(path: Option<&str>) -> Result<Self, ConfigError> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        if path.is_empty() || path.len() > 1024 {
            return Err(ConfigError("MCP 能力文件路径无效"));
        }
        let file = std::fs::File::open(Path::new(path))
            .map_err(|_| ConfigError("MCP 部署能力文件不可读"))?;
        let mut bytes = Vec::new();
        file.take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| ConfigError("MCP 部署能力文件不可读"))?;
        if bytes.len() > 65536 {
            return Err(ConfigError("MCP 部署能力文件过大"));
        }
        let file: File =
            serde_json::from_slice(&bytes).map_err(|_| ConfigError("MCP 部署能力格式无效"))?;
        if file.format_version != 1 || file.programs.len() > 16 {
            return Err(ConfigError("MCP 部署能力版本或数量无效"));
        }
        let mut programs = BTreeMap::new();
        for value in file.programs {
            if !identifier(&value.id)
                || !text(&value.label, 80)
                || !value.program.is_absolute()
                || !value.program.is_file()
                || !value.working_directory.is_absolute()
                || !value.working_directory.is_dir()
                || value.arguments.len() > 64
                || value.arguments.iter().any(|v| v.contains('\0'))
                || value.arguments.iter().map(String::len).sum::<usize>() > 16384
                || value.environment.len() > 32
                || value.environment.iter().any(|(k, v)| {
                    k.is_empty()
                        || k.len() > 128
                        || !k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                        || v.contains('\0')
                })
                || value
                    .environment
                    .iter()
                    .map(|(k, v)| k.len() + v.len())
                    .sum::<usize>()
                    > 16384
                || value.allowed_tools.is_empty()
                || value.allowed_tools.len() > 128
                || value.allowed_tools.iter().any(|v| !text(v, 128))
                || value.allowed_tools.iter().collect::<BTreeSet<_>>().len()
                    != value.allowed_tools.len()
                || !(100..=300_000).contains(&value.startup_timeout_ms)
                || !(50..=300_000).contains(&value.request_timeout_ms)
                || !(100..=30_000).contains(&value.stop_timeout_ms)
                || programs.contains_key(&value.id)
            {
                return Err(ConfigError("MCP 部署能力字段或限制无效"));
            }
            programs.insert(value.id.clone(), value);
        }
        Ok(Self { programs })
    }
    pub fn get(&self, id: &str) -> Option<&Capability> {
        self.programs.get(id)
    }
    pub fn views(&self) -> Vec<CapabilityView> {
        self.programs
            .values()
            .map(|v| CapabilityView {
                id: v.id.clone(),
                label: v.label.clone(),
                allowed_tools: v.allowed_tools.clone(),
            })
            .collect()
    }
}
impl Capability {
    pub fn fingerprint(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).expect("typed capability"))
        )
    }
    pub fn spec(&self) -> ProgramSpec {
        ProgramSpec {
            program: self.program.clone(),
            args: self.arguments.clone(),
            cwd: self.working_directory.clone(),
            environment: self.environment.clone(),
        }
    }
    pub fn limits(&self) -> Limits {
        Limits {
            startup_timeout: Duration::from_millis(self.startup_timeout_ms),
            request_timeout: Duration::from_millis(self.request_timeout_ms),
            stop_timeout: Duration::from_millis(self.stop_timeout_ms),
            ..Default::default()
        }
    }
}
