use crate::common::error::AppError;
pub use crate::modules::codegen::dto::{DeleteQuery, Preview, request_id, version};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tri_extension_package::package::{self, File, Package, SkillMeta};
fn mode() -> u32 {
    0o644
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputFile {
    pub path: String,
    pub content_base64: String,
    #[serde(default = "mode")]
    pub mode: u32,
}
fn package_error(error: package::Error) -> AppError {
    AppError::Validation(error.to_string())
}
impl InputFile {
    fn decode(self) -> Result<File, AppError> {
        if self.content_base64.len() > package::MAX_FILE.div_ceil(3) * 4 {
            return Err(AppError::PayloadTooLarge);
        }
        let bytes = STANDARD
            .decode(self.content_base64)
            .map_err(|_| AppError::Validation("资源编码无效".into()))?;
        File::new(self.path, bytes, self.mode).map_err(package_error)
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Source {
    Archive {
        #[serde(rename = "contentBase64")]
        content_base64: String,
    },
    Skill {
        meta: SkillMeta,
        markdown: String,
        #[serde(default)]
        files: Vec<InputFile>,
    },
    Files {
        write: Vec<InputFile>,
        remove: Vec<String>,
    },
}
impl Source {
    pub fn build(self, base: Option<Package>) -> Result<Package, AppError> {
        match self {
            Self::Archive { content_base64 } => {
                if content_base64.len() > package::MAX_ARCHIVE.div_ceil(3) * 4 {
                    return Err(AppError::PayloadTooLarge);
                }
                let bytes = STANDARD
                    .decode(content_base64)
                    .map_err(|_| AppError::Validation("扩展包编码无效".into()))?;
                package::parse(&bytes).map_err(package_error)
            }
            Self::Skill {
                meta,
                markdown,
                files,
            } => {
                if files.len() >= package::MAX_FILES {
                    return Err(AppError::Validation("配套文件数量过多".into()));
                }
                package::author_skill(
                    meta,
                    markdown,
                    files
                        .into_iter()
                        .map(InputFile::decode)
                        .collect::<Result<_, _>>()?,
                )
                .map_err(package_error)
            }
            Self::Files { write, remove } => {
                let base =
                    base.ok_or_else(|| AppError::Validation("新建扩展不能使用文件修订".into()))?;
                if write.len() > package::MAX_FILES
                    || remove.len() > package::MAX_FILES
                    || write.is_empty() && remove.is_empty()
                {
                    return Err(AppError::Validation("文件修订无效".into()));
                }
                let mut names = BTreeSet::new();
                for path in write.iter().map(|v| &v.path).chain(remove.iter()) {
                    if !package::valid_path(path) || !names.insert(path.to_lowercase()) {
                        return Err(AppError::Validation("文件路径重复或无效".into()));
                    }
                }
                let mut files = base
                    .files
                    .into_iter()
                    .map(|file| (file.path.clone(), file))
                    .collect::<BTreeMap<_, _>>();
                for path in remove {
                    if files.remove(&path).is_none() {
                        return Err(AppError::Validation("待删除文件不存在".into()));
                    }
                }
                for file in write {
                    let file = file.decode()?;
                    files.insert(file.path.clone(), file);
                }
                package::repack(&base.name, files.into_values().collect()).map_err(package_error)
            }
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Install {
    pub request_id: String,
    pub source: Source,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Revise {
    pub request_id: String,
    pub version: i64,
    pub source: Source,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Toggle {
    pub version: i64,
    pub enabled: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConstraintInput {
    pub version: i64,
    pub content: String,
}
fn first() -> u64 {
    1
}
fn size() -> u64 {
    20
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LibraryStatus {
    #[default]
    Active,
    Uninstalled,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryQuery {
    #[serde(default = "first")]
    pub page: u64,
    #[serde(default = "size")]
    pub page_size: u64,
    pub search: Option<String>,
    pub sort: Option<String>,
    #[serde(default)]
    pub status: LibraryStatus,
}
