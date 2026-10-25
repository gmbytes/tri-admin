//! Disk I/O for paths already selected by the user. Paths are never serialized to the webview.
mod handles;
use crate::{DesktopError, error};
use cap_std::fs::{Dir, OpenOptions};
pub use handles::{FileHandles, FileJobKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

pub const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024 * 1024;
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub const WRITE_BLOCK: usize = 64 * 1024;
fn io_error() -> DesktopError {
    error("FILE_IO", "无法读写选定文件")
}
fn changed() -> DesktopError {
    error("FILE_SOURCE_CHANGED", "源文件已变更，请重新选择")
}
fn integrity() -> DesktopError {
    error("FILE_INTEGRITY", "文件长度或摘要不匹配，未保存目标文件")
}

#[derive(Clone, Default)]
pub struct FileCancel {
    cancelled: Arc<AtomicBool>,
    commit: Arc<Mutex<()>>,
}
impl FileCancel {
    pub fn cancel(&self) {
        let _guard = self
            .commit
            .lock()
            .unwrap_or_else(|failure| failure.into_inner());
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), DesktopError> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(error("REQUEST_CANCELLED", "文件操作已取消"))
        } else {
            Ok(())
        }
    }
    fn publish<T>(
        &self,
        operation: impl FnOnce() -> Result<T, DesktopError>,
    ) -> Result<T, DesktopError> {
        let _guard = self.commit.lock().map_err(|_| io_error())?;
        self.check()?;
        operation()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileSpec {
    size: u64,
    sha256: String,
}
impl FileSpec {
    pub fn new(size: u64, sha256: impl Into<String>) -> Result<Self, DesktopError> {
        let value = Self {
            size,
            sha256: sha256.into(),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn validate(&self) -> Result<(), DesktopError> {
        if self.size > MAX_FILE_SIZE
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
        {
            Err(error("FILE_METADATA_INVALID", "文件长度或摘要无效"))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, PartialEq)]
struct Fingerprint {
    size: u64,
    modified: SystemTime,
}
fn fingerprint(file: &File) -> Result<Fingerprint, DesktopError> {
    let metadata = file.metadata().map_err(|_| io_error())?;
    if !metadata.is_file() {
        return Err(error("FILE_SOURCE_INVALID", "请选择普通文件"));
    }
    Ok(Fingerprint {
        size: metadata.len(),
        modified: metadata.modified().map_err(|_| io_error())?,
    })
}
pub struct NativeSource {
    file: File,
    info: SourceInfo,
    fingerprint: Fingerprint,
    cancel: FileCancel,
}
impl NativeSource {
    pub fn open(path: &Path, cancel: &FileCancel) -> Result<Self, DesktopError> {
        Self::open_with_progress(path, cancel, |_| {})
    }
    pub fn open_with_progress(
        path: &Path,
        cancel: &FileCancel,
        mut progress: impl FnMut(u64),
    ) -> Result<Self, DesktopError> {
        cancel.check()?;
        if !path.is_absolute() || !std::fs::metadata(path).map_err(|_| io_error())?.is_file() {
            return Err(error("FILE_SOURCE_INVALID", "请选择普通文件"));
        }
        let mut file = File::open(path).map_err(|_| io_error())?;
        let initial = fingerprint(&file)?;
        if initial.size > MAX_FILE_SIZE {
            return Err(error("FILE_LIMIT", "文件最多为 5 GiB"));
        }
        let mut hash = Sha256::new();
        let mut count = 0u64;
        let mut buffer = [0u8; WRITE_BLOCK];
        loop {
            cancel.check()?;
            let length = file.read(&mut buffer).map_err(|_| io_error())?;
            if length == 0 {
                break;
            }
            count = count.checked_add(length as u64).ok_or_else(changed)?;
            if count > initial.size {
                return Err(changed());
            }
            hash.update(&buffer[..length]);
            progress(count);
        }
        cancel.check()?;
        if count != initial.size || fingerprint(&file)? != initial {
            return Err(changed());
        }
        file.rewind().map_err(|_| io_error())?;
        let name = path
            .file_name()
            .ok_or_else(changed)?
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            file,
            info: SourceInfo {
                name,
                size: count,
                sha256: format!("{:x}", hash.finalize()),
            },
            fingerprint: initial,
            cancel: cancel.clone(),
        })
    }
    pub fn info(&self) -> &SourceInfo {
        &self.info
    }
    pub fn read_chunk(&mut self, index: u32) -> Result<Vec<u8>, DesktopError> {
        self.cancel.check()?;
        let offset = u64::from(index) * CHUNK_SIZE as u64;
        if offset >= self.info.size || offset >= MAX_FILE_SIZE {
            return Err(error("FILE_CHUNK_INVALID", "分片索引无效"));
        }
        if fingerprint(&self.file)? != self.fingerprint {
            return Err(changed());
        }
        let length = (self.info.size - offset).min(CHUNK_SIZE as u64) as usize;
        let mut bytes = vec![0u8; length];
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| io_error())?;
        self.file.read_exact(&mut bytes).map_err(|_| changed())?;
        self.cancel.check()?;
        if fingerprint(&self.file)? != self.fingerprint {
            return Err(changed());
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy)]
pub enum SaveMode {
    New,
    Replace,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReceipt {
    pub size: u64,
    pub sha256: String,
    pub cleanup_pending: bool,
}
pub struct NativeWriter {
    directory: Dir,
    target: OsString,
    temporary: String,
    file: Option<File>,
    spec: FileSpec,
    hash: Sha256,
    written: u64,
    failed: bool,
    mode: SaveMode,
    cancel: FileCancel,
}
impl NativeWriter {
    pub fn prepare(
        path: &Path,
        spec: FileSpec,
        mode: SaveMode,
        cancel: &FileCancel,
    ) -> Result<Self, DesktopError> {
        spec.validate()?;
        cancel.check()?;
        if !path.is_absolute() {
            return Err(error("FILE_DESTINATION_INVALID", "请选择有效保存位置"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| error("FILE_DESTINATION_INVALID", "请选择有效保存位置"))?;
        let target = path
            .file_name()
            .ok_or_else(|| error("FILE_DESTINATION_INVALID", "请选择有效文件名"))?
            .to_os_string();
        let directory =
            Dir::open_ambient_dir(parent, cap_std::ambient_authority()).map_err(|_| io_error())?;
        match directory.symlink_metadata(&target) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(error("FILE_DESTINATION_INVALID", "保存目标不是普通文件"));
                }
                if matches!(mode, SaveMode::New) {
                    return Err(error("FILE_EXISTS", "目标文件已存在，请重新选择"));
                }
            }
            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(io_error()),
        }
        let temporary = format!(".tri-admin-download-{}.part", uuid::Uuid::new_v4());
        let file = directory
            .open_with(
                &temporary,
                OpenOptions::new().write(true).read(true).create_new(true),
            )
            .map_err(|_| io_error())?
            .into_std();
        Ok(Self {
            directory,
            target,
            temporary,
            file: Some(file),
            spec,
            hash: Sha256::new(),
            written: 0,
            failed: false,
            mode,
            cancel: cancel.clone(),
        })
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), DesktopError> {
        self.cancel.check()?;
        if self.failed {
            return Err(io_error());
        }
        if bytes.len() > WRITE_BLOCK {
            return Err(error("FILE_BLOCK_INVALID", "文件写入块超过 64 KiB"));
        }
        let count = self
            .written
            .checked_add(bytes.len() as u64)
            .ok_or_else(integrity)?;
        if count > self.spec.size {
            self.failed = true;
            return Err(integrity());
        }
        if self
            .file
            .as_mut()
            .ok_or_else(io_error)?
            .write_all(bytes)
            .is_err()
        {
            self.failed = true;
            return Err(io_error());
        }
        self.hash.update(bytes);
        self.written = count;
        self.cancel.check()
    }
    pub fn complete(mut self) -> Result<SaveReceipt, DesktopError> {
        self.cancel.check()?;
        if self.failed
            || self.written != self.spec.size
            || format!("{:x}", self.hash.clone().finalize()) != self.spec.sha256
        {
            return Err(integrity());
        }
        let mut file = self.file.take().ok_or_else(io_error)?;
        file.flush().map_err(|_| io_error())?;
        file.sync_all().map_err(|_| io_error())?;
        drop(file);
        self.cancel.publish(|| {
            match self.mode {
                SaveMode::New => {
                    self.directory
                        .hard_link(&self.temporary, &self.directory, &self.target)
                }
                SaveMode::Replace => {
                    self.directory
                        .rename(&self.temporary, &self.directory, &self.target)
                }
            }
            .map_err(|failure| {
                if failure.kind() == std::io::ErrorKind::AlreadyExists {
                    error("FILE_EXISTS", "目标文件已存在，请重新选择")
                } else {
                    io_error()
                }
            })
        })?;
        let cleanup_pending = self.remove_temp().is_err();
        Ok(SaveReceipt {
            size: self.spec.size,
            sha256: self.spec.sha256.clone(),
            cleanup_pending,
        })
    }
    pub fn abort(mut self) -> Result<(), DesktopError> {
        self.cancel.cancel();
        drop(self.file.take());
        self.remove_temp()
            .map_err(|_| error("FILE_CLEANUP_PENDING", "临时文件清理待重试"))
    }
    fn remove_temp(&self) -> std::io::Result<()> {
        match self.directory.remove_file(&self.temporary) {
            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}
impl Drop for NativeWriter {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = self.remove_temp();
    }
}
