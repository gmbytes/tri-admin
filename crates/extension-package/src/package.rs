use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
};
use unicode_normalization::UnicodeNormalization;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};
pub const MAX_ARCHIVE: usize = 4 * 1024 * 1024;
pub const MAX_EXPANDED: usize = 16 * 1024 * 1024;
pub const MAX_FILE: usize = 4 * 1024 * 1024;
pub const MAX_FILES: usize = 256;
pub const MAX_TEXT: usize = 256 * 1024;
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("扩展包格式无效")]
    Archive,
    #[error("扩展文件路径或类型无效")]
    Path,
    #[error("扩展内容超过大小或数量限制")]
    Limit,
    #[error("Skill 元数据或正文无效")]
    Skill,
    #[error("插件描述或贡献引用无效")]
    Plugin,
    #[error("此资源不能作为文本预览")]
    Text,
}
type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Skill,
    Plugin,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub allowed_tools: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Skill {
    pub meta: SkillMeta,
    pub entry_path: String,
    pub markdown: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub meta: SkillMeta,
    pub entry_path: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpContribution {
    pub id: String,
    pub label: String,
    pub capability_id: String,
    pub tools: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolBinding {
    pub name: String,
    pub description: String,
    pub operation: String,
    pub input_schema: Value,
    pub mappings: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginManifest {
    format_version: u32,
    name: String,
    version: String,
    description: String,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    mcp: Vec<McpContribution>,
    #[serde(default)]
    tools: Vec<ToolBinding>,
}
#[derive(Clone, Debug)]
pub struct File {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    pub mode: u32,
    pub sha256: String,
    pub text: bool,
}
#[derive(Clone, Debug)]
pub struct Package {
    pub kind: Kind,
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub skills: Vec<Skill>,
    pub mcp: Vec<McpContribution>,
    pub tools: Vec<ToolBinding>,
    pub files: Vec<File>,
    pub archive: Vec<u8>,
    pub archive_sha256: String,
    pub tree_sha256: String,
    pub expanded_size: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInfo {
    pub kind: Kind,
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub skills: Vec<SkillInfo>,
    pub mcp: Vec<McpContribution>,
    pub tools: Vec<ToolBinding>,
    pub files: Vec<FileInfo>,
    pub archive_sha256: String,
    pub tree_sha256: String,
    pub archive_size: u64,
    pub expanded_size: u64,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max
        && !value
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}
pub fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
pub fn valid_path(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 240
        || value.starts_with('/')
        || value.ends_with('/')
        || !value.chars().eq(value.nfc())
        || value.split('/').count() > 16
    {
        return false;
    }
    value.split('/').all(|part| {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        !part.is_empty()
            && part != "."
            && part != ".."
            && part.len() <= 100
            && !part.ends_with('.')
            && !part.ends_with(' ')
            && !part
                .chars()
                .any(|c| c.is_control() || "<>:\"\\|?*".contains(c))
            && !matches!(
                part.to_ascii_lowercase().as_str(),
                ".git" | ".svn" | ".hg" | ".ssh" | ".env"
            )
            && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !(stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    })
}
impl File {
    pub fn new(path: String, bytes: Vec<u8>, mode: u32) -> Result<Self> {
        if !valid_path(&path) || mode > 0o777 {
            return Err(Error::Path);
        }
        if bytes.len() > MAX_FILE {
            return Err(Error::Limit);
        }
        let sha256 = hash(&bytes);
        Ok(Self {
            path,
            bytes,
            mode,
            sha256,
        })
    }
    pub fn text(&self) -> Result<&str> {
        if self.bytes.len() > MAX_TEXT || self.bytes.contains(&0) {
            return Err(Error::Text);
        }
        std::str::from_utf8(&self.bytes).map_err(|_| Error::Text)
    }
    pub fn info(&self) -> FileInfo {
        FileInfo {
            path: self.path.clone(),
            size: self.bytes.len() as u64,
            mode: self.mode,
            sha256: self.sha256.clone(),
            text: self.text().is_ok(),
        }
    }
}
impl SkillMeta {
    pub fn validate(&self) -> Result<()> {
        if !valid_name(&self.name)
            || !text(&self.description, 1024)
            || self.license.as_ref().is_some_and(|v| !text(v, 256))
            || self.compatibility.as_ref().is_some_and(|v| !text(v, 500))
            || self.allowed_tools.as_ref().is_some_and(|v| !text(v, 2048))
            || self.metadata.len() > 64
            || self
                .metadata
                .iter()
                .any(|(key, value)| !text(key, 64) || !text(value, 1024))
        {
            return Err(Error::Skill);
        }
        Ok(())
    }
}
impl Package {
    pub fn info(&self) -> PackageInfo {
        PackageInfo {
            kind: self.kind,
            name: self.name.clone(),
            version: self.version.clone(),
            description: self.description.clone(),
            skills: self
                .skills
                .iter()
                .map(|s| SkillInfo {
                    meta: s.meta.clone(),
                    entry_path: s.entry_path.clone(),
                    sha256: s.sha256.clone(),
                })
                .collect(),
            mcp: self.mcp.clone(),
            tools: self.tools.clone(),
            files: self.files.iter().map(File::info).collect(),
            archive_sha256: self.archive_sha256.clone(),
            tree_sha256: self.tree_sha256.clone(),
            archive_size: self.archive.len() as u64,
            expanded_size: self.expanded_size,
        }
    }
}
fn parse_skill(file: &File, parent: &str) -> Result<Skill> {
    let value = file.text().map_err(|_| Error::Skill)?;
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(Error::Skill);
    }
    let mut lines = value.split_inclusive('\n');
    let first = lines.next().ok_or(Error::Skill)?;
    if first.trim_end_matches(['\r', '\n', ' ']) != "---" {
        return Err(Error::Skill);
    }
    let mut offset = first.len();
    let start = offset;
    let mut boundary = None;
    for line in lines {
        if line.trim_end_matches(['\r', '\n', ' ']) == "---" {
            boundary = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
        if offset - start > 16 * 1024 {
            return Err(Error::Limit);
        }
    }
    let (end, body) = boundary.ok_or(Error::Skill)?;
    if end - start > 16 * 1024 {
        return Err(Error::Limit);
    }
    let options = serde_saphyr::options! {budget:serde_saphyr::budget!{max_documents:1,max_depth:8,max_nodes:512,max_events:1024,max_aliases:0,max_anchors:0,max_total_scalar_bytes:32*1024,max_inclusion_depth:0,max_merge_keys:0},reject_unsupported_tags:true,no_schema:true,strict_booleans:true,merge_keys:serde_saphyr::options::MergeKeyPolicy::Error,with_snippet:false};
    let parsed: Frontmatter = serde_saphyr::from_str_with_options(&value[start..end], options)
        .map_err(|_| Error::Skill)?;
    let meta = SkillMeta {
        name: parsed.name,
        description: parsed.description,
        license: parsed.license,
        compatibility: parsed.compatibility,
        metadata: parsed.metadata,
        allowed_tools: parsed.allowed_tools,
    };
    meta.validate()?;
    if meta.name != parent {
        return Err(Error::Skill);
    }
    Ok(Skill {
        meta,
        entry_path: file.path.clone(),
        markdown: value[body..].into(),
        sha256: file.sha256.clone(),
    })
}
fn validate_manifest(manifest: &PluginManifest, root: &str) -> Result<()> {
    if manifest.format_version != 1
        || manifest.name != root
        || !valid_name(&manifest.name)
        || manifest.version.len() > 64
        || semver::Version::parse(&manifest.version).is_err()
        || !text(&manifest.description, 1024)
        || manifest.skills.len() > 32
        || manifest.mcp.len() > 16
        || manifest.tools.len() > 64
    {
        return Err(Error::Plugin);
    }
    let mut names = BTreeSet::new();
    for path in &manifest.skills {
        if !valid_path(path) || !names.insert(path.to_lowercase()) {
            return Err(Error::Plugin);
        }
    }
    let mut names = BTreeSet::new();
    for mcp in &manifest.mcp {
        if !valid_name(&mcp.id)
            || !valid_name(&mcp.capability_id)
            || !text(&mcp.label, 80)
            || mcp.tools.len() > 64
            || !names.insert(&mcp.id)
            || mcp.tools.iter().any(|v| !identifier(v))
            || mcp.tools.iter().collect::<BTreeSet<_>>().len() != mcp.tools.len()
        {
            return Err(Error::Plugin);
        }
    }
    let mut names = BTreeSet::new();
    for tool in &manifest.tools {
        if !identifier(&tool.name)
            || !names.insert(&tool.name)
            || !text(&tool.description, 1024)
            || tool.operation.is_empty()
            || tool.operation.len() > 80
            || !tool
                .operation
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._".contains(&c))
            || tool.input_schema.get("type").and_then(Value::as_str) != Some("object")
            || tool.mappings.len() > 64
        {
            return Err(Error::Plugin);
        }
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or(Error::Plugin)?;
        if properties.len() > 64
            || tool.mappings.iter().any(|(target, source)| {
                !identifier(target) || !identifier(source) || !properties.contains_key(source)
            })
        {
            return Err(Error::Plugin);
        }
    }
    Ok(())
}
pub fn parse(bytes: &[u8]) -> Result<Package> {
    if bytes.len() > MAX_ARCHIVE {
        return Err(Error::Limit);
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| Error::Archive)?;
    if archive.is_empty() || archive.len() > 512 {
        return Err(Error::Limit);
    }
    let mut root = None::<String>;
    let mut files = Vec::new();
    let mut names = BTreeSet::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|_| Error::Archive)?;
        let name = std::str::from_utf8(entry.name_raw())
            .map_err(|_| Error::Path)?
            .to_owned();
        if name != entry.name().map_err(|_| Error::Path)? {
            return Err(Error::Path);
        }
        let directory = entry.is_dir();
        let normalized = if directory {
            name.strip_suffix('/').ok_or(Error::Path)?
        } else {
            name.as_str()
        };
        if !valid_path(normalized) || !names.insert(normalized.to_lowercase()) {
            return Err(Error::Path);
        }
        let (first, relative) = normalized.split_once('/').unwrap_or((normalized, ""));
        if !valid_name(first) {
            return Err(Error::Path);
        }
        if let Some(root) = &root {
            if root != first {
                return Err(Error::Path);
            }
        } else {
            root = Some(first.to_owned())
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if mode & 0o6000 != 0
                || !matches!(kind, 0 | 0o100000 | 0o040000)
                || directory && kind == 0o100000
                || !directory && kind == 0o040000
            {
                return Err(Error::Path);
            }
        }
        if directory {
            continue;
        }
        if relative.is_empty() || files.len() >= MAX_FILES || entry.size() > MAX_FILE as u64 {
            return Err(Error::Limit);
        }
        total = total
            .checked_add(entry.size() as usize)
            .filter(|v| *v <= MAX_EXPANDED)
            .ok_or(Error::Limit)?;
        let mode = entry.unix_mode().map(|mode| mode & 0o777).unwrap_or(0o644);
        let mut data = Vec::new();
        (&mut entry)
            .take(MAX_FILE as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| Error::Archive)?;
        if data.len() as u64 != entry.size() || data.len() > MAX_FILE {
            return Err(Error::Limit);
        }
        files.push(File::new(relative.into(), data, mode)?);
    }
    if files.is_empty() {
        return Err(Error::Archive);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let paths = files
        .iter()
        .map(|file| file.path.to_lowercase())
        .collect::<BTreeSet<_>>();
    for path in &paths {
        let mut prefix = String::new();
        for component in path.split('/').take(path.split('/').count() - 1) {
            if !prefix.is_empty() {
                prefix.push('/')
            }
            prefix.push_str(component);
            if paths.contains(&prefix) {
                return Err(Error::Path);
            }
        }
    }
    let root = root.ok_or(Error::Archive)?;
    let skill = files.iter().find(|file| file.path == "SKILL.md");
    let plugin = files.iter().find(|file| file.path == "tri-plugin.json");
    let (kind, name, version, description, skills, mcp, tools) = match (skill, plugin) {
        (Some(file), None) => {
            let skill = parse_skill(file, &root)?;
            let version = skill.meta.metadata.get("version").cloned();
            (
                Kind::Skill,
                root,
                version,
                skill.meta.description.clone(),
                vec![skill],
                Vec::new(),
                Vec::new(),
            )
        }
        (None, Some(file)) => {
            let content = file.text().map_err(|_| Error::Plugin)?;
            let manifest: PluginManifest =
                serde_json::from_str(content).map_err(|_| Error::Plugin)?;
            validate_manifest(&manifest, &root)?;
            let mut skills = Vec::new();
            for path in &manifest.skills {
                let entry = format!("{path}/SKILL.md");
                let file = files
                    .iter()
                    .find(|file| file.path == entry)
                    .ok_or(Error::Plugin)?;
                skills.push(parse_skill(
                    file,
                    path.rsplit('/').next().ok_or(Error::Plugin)?,
                )?)
            }
            (
                Kind::Plugin,
                manifest.name,
                Some(manifest.version),
                manifest.description,
                skills,
                manifest.mcp,
                manifest.tools,
            )
        }
        _ => return Err(Error::Archive),
    };
    let canonical = files
        .iter()
        .map(|file| (&file.path, file.mode, &file.sha256))
        .collect::<Vec<_>>();
    let tree_sha256 = hash(&serde_json::to_vec(&canonical).map_err(|_| Error::Archive)?);
    Ok(Package {
        kind,
        name,
        version,
        description,
        skills,
        mcp,
        tools,
        files,
        archive: bytes.to_vec(),
        archive_sha256: hash(bytes),
        tree_sha256,
        expanded_size: total as u64,
    })
}
pub fn repack(root: &str, files: Vec<File>) -> Result<Package> {
    if !valid_name(root) || files.is_empty() || files.len() > MAX_FILES {
        return Err(Error::Path);
    }
    let mut safe = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut total = 0usize;
    for file in files {
        let file = File::new(file.path, file.bytes, file.mode)?;
        total = total
            .checked_add(file.bytes.len())
            .filter(|v| *v <= MAX_EXPANDED)
            .ok_or(Error::Limit)?;
        if !names.insert(file.path.to_lowercase()) {
            return Err(Error::Path);
        }
        safe.insert(file.path.clone(), file);
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for file in safe.into_values() {
        zip.start_file(
            format!("{root}/{}", file.path),
            SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(zip::DateTime::default())
                .unix_permissions(file.mode),
        )
        .map_err(|_| Error::Archive)?;
        zip.write_all(&file.bytes).map_err(|_| Error::Archive)?;
    }
    let bytes = zip.finish().map_err(|_| Error::Archive)?.into_inner();
    parse(&bytes)
}
pub fn author_skill(meta: SkillMeta, markdown: String, mut files: Vec<File>) -> Result<Package> {
    meta.validate()?;
    if markdown.len() > MAX_TEXT {
        return Err(Error::Limit);
    }
    let mut header = BTreeMap::<&str, Value>::new();
    header.insert("name", Value::String(meta.name.clone()));
    header.insert("description", Value::String(meta.description.clone()));
    if let Some(value) = &meta.license {
        header.insert("license", Value::String(value.clone()));
    }
    if let Some(value) = &meta.compatibility {
        header.insert("compatibility", Value::String(value.clone()));
    }
    if let Some(value) = &meta.allowed_tools {
        header.insert("allowed-tools", Value::String(value.clone()));
    }
    if !meta.metadata.is_empty() {
        header.insert(
            "metadata",
            serde_json::to_value(&meta.metadata).map_err(|_| Error::Skill)?,
        );
    }
    let mut body = String::from("---\n");
    for (key, value) in header {
        body.push_str(key);
        body.push_str(": ");
        body.push_str(&serde_json::to_string(&value).map_err(|_| Error::Skill)?);
        body.push('\n')
    }
    body.push_str("---\n");
    body.push_str(&markdown);
    files.push(File::new("SKILL.md".into(), body.into_bytes(), 0o644)?);
    repack(&meta.name, files)
}
