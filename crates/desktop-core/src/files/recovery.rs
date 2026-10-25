use crate::{DesktopError, error};
use cap_fs_ext::MetadataExt;
use cap_std::fs::{Dir, Metadata, OpenOptions};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
const MAX_RECORD_BYTES: u64 = 8192;
fn unavailable() -> DesktopError {
    error("FILE_JOURNAL_UNAVAILABLE", "本机文件恢复记录暂不可用")
}
fn pending() -> std::io::Error {
    std::io::Error::other("native cleanup remains pending")
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
    birth: String,
}
fn identity(metadata: &Metadata) -> std::io::Result<Identity> {
    let created = metadata.created()?.into_std();
    let birth = match created.duration_since(std::time::UNIX_EPOCH) {
        Ok(value) => value.as_nanos().to_string(),
        Err(value) => format!("-{}", value.duration().as_nanos()),
    };
    Ok(Identity {
        device: metadata.dev(),
        inode: metadata.ino(),
        birth,
    })
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Reservation {
    schema_version: u32,
    id: String,
    parent: PathBuf,
    parent_identity: Identity,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Owned {
    schema_version: u32,
    id: String,
    identity: Identity,
}
#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRecoveryReport {
    pub scanned: u64,
    pub cleaned: u64,
    pub active: u64,
    pub pending: u64,
    pub invalid: u64,
}
#[derive(Clone)]
pub struct NativeFileJournal {
    directory: Arc<Dir>,
    allocation: Arc<Mutex<()>>,
    root: Arc<PathBuf>,
}
fn valid_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
fn record_name(id: &str) -> String {
    format!("{id}.journal")
}
fn part_name(id: &str) -> String {
    format!(".tri-admin-download-{id}.part")
}
fn write_line<T: Serialize>(file: &mut File, value: &T) -> Result<(), DesktopError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| unavailable())?;
    bytes.push(b'\n');
    if file.metadata().map_err(|_| unavailable())?.len() + bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(error(
            "FILE_DESTINATION_INVALID",
            "保存位置过长，无法可靠记录恢复信息",
        ));
    }
    file.write_all(&bytes).map_err(|_| unavailable())?;
    file.sync_all().map_err(|_| unavailable())
}
impl NativeFileJournal {
    pub fn open(root: &Path) -> Result<Self, DesktopError> {
        if !root.is_absolute() {
            return Err(unavailable());
        }
        std::fs::create_dir_all(root).map_err(|_| unavailable())?;
        let root = root.canonicalize().map_err(|_| unavailable())?;
        let directory = Dir::open_ambient_dir(&root, cap_std::ambient_authority())
            .map_err(|_| unavailable())?;
        Ok(Self {
            directory: Arc::new(directory),
            allocation: Arc::new(Mutex::new(())),
            root: Arc::new(root),
        })
    }
    pub(super) fn allocate(
        &self,
        parent: PathBuf,
        directory: Arc<Dir>,
        target: &OsStr,
    ) -> Result<(NativePartLease, File), DesktopError> {
        if parent.starts_with(self.root.as_path())
            || identity(&directory.dir_metadata().map_err(|_| unavailable())?)
                .map_err(|_| unavailable())?
                == identity(&self.directory.dir_metadata().map_err(|_| unavailable())?)
                    .map_err(|_| unavailable())?
        {
            return Err(error(
                "FILE_DESTINATION_INVALID",
                "不能将文件保存到应用恢复目录",
            ));
        }
        let _guard = self.allocation.lock().map_err(|_| unavailable())?;
        let mut count = 0;
        for row in self
            .directory
            .entries()
            .map_err(|_| unavailable())?
            .take(1001)
        {
            if row
                .map_err(|_| unavailable())?
                .file_name()
                .to_string_lossy()
                .ends_with(".journal")
            {
                count += 1
            }
        }
        if count >= 512 {
            return Err(error(
                "FILE_RECOVERY_CAPACITY",
                "本机待处理文件记录过多，请先完成临时清理",
            ));
        }
        let id = loop {
            let id = uuid::Uuid::new_v4().to_string();
            if !target
                .to_string_lossy()
                .eq_ignore_ascii_case(&part_name(&id))
            {
                break id;
            }
        };
        let parent_identity = identity(&directory.dir_metadata().map_err(|_| unavailable())?)
            .map_err(|_| unavailable())?;
        let reservation = Reservation {
            schema_version: 1,
            id: id.clone(),
            parent,
            parent_identity,
        };
        let lease = self
            .directory
            .open_with(
                record_name(&id),
                OpenOptions::new().read(true).write(true).create_new(true),
            )
            .map_err(|_| unavailable())?
            .into_std();
        if lease.try_lock().is_err() {
            drop(lease);
            let _ = self.directory.remove_file(record_name(&id));
            return Err(unavailable());
        }
        let mut ticket = NativePartLease {
            journal: self.directory.clone(),
            directory,
            id,
            lease: Some(lease),
            identity: None,
            created: false,
        };
        write_line(ticket.lease.as_mut().ok_or_else(unavailable)?, &reservation)?;
        let file = ticket
            .directory
            .open_with(
                ticket.part_name(),
                OpenOptions::new().read(true).write(true).create_new(true),
            )
            .map_err(|_| super::io_error())?;
        ticket.created = true;
        let file_identity =
            identity(&file.metadata().map_err(|_| unavailable())?).map_err(|_| unavailable())?;
        ticket.identity = Some(file_identity.clone());
        write_line(
            ticket.lease.as_mut().ok_or_else(unavailable)?,
            &Owned {
                schema_version: 1,
                id: ticket.id.clone(),
                identity: file_identity,
            },
        )?;
        Ok((ticket, file.into_std()))
    }
    pub fn recover(&self) -> Result<NativeRecoveryReport, DesktopError> {
        let mut result = NativeRecoveryReport::default();
        for row in self
            .directory
            .entries()
            .map_err(|_| unavailable())?
            .take(1000)
        {
            if result.cleaned >= 100 {
                break;
            }
            let row = match row {
                Ok(row) => row,
                Err(_) => {
                    result.invalid += 1;
                    continue;
                }
            };
            let name = row.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(id) = name.strip_suffix(".journal") else {
                continue;
            };
            if !valid_id(id) {
                continue;
            }
            result.scanned += 1;
            let metadata = match self.directory.symlink_metadata(name) {
                Ok(value) => value,
                Err(_) => {
                    result.pending += 1;
                    continue;
                }
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                result.invalid += 1;
                continue;
            }
            let mut file = match self
                .directory
                .open_with(name, OpenOptions::new().read(true).write(true))
            {
                Ok(file) => file.into_std(),
                Err(_) => {
                    result.pending += 1;
                    continue;
                }
            };
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => {
                    result.active += 1;
                    continue;
                }
                Err(_) => {
                    result.pending += 1;
                    continue;
                }
            }
            let record = read_record(&mut file, id);
            let (reservation, owned) = match record {
                Ok(record) => record,
                Err(_) => {
                    result.invalid += 1;
                    continue;
                }
            };
            let directory =
                match Dir::open_ambient_dir(&reservation.parent, cap_std::ambient_authority()) {
                    Ok(value) => value,
                    Err(_) => {
                        result.pending += 1;
                        continue;
                    }
                };
            if directory
                .dir_metadata()
                .and_then(|metadata| identity(&metadata))
                .ok()
                .as_ref()
                != Some(&reservation.parent_identity)
            {
                result.pending += 1;
                continue;
            }
            if cleanup_part(&directory, &part_name(id), owned.as_ref()).is_err() {
                result.pending += 1;
                continue;
            }
            if self.directory.remove_file(name).is_ok() {
                result.cleaned += 1
            } else {
                result.pending += 1
            }
        }
        Ok(result)
    }
}
fn read_record(file: &mut File, id: &str) -> std::io::Result<(Reservation, Option<Identity>)> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    Read::by_ref(file)
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(pending());
    }
    let lines = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() || lines.len() > 2 {
        return Err(pending());
    }
    let reservation: Reservation = serde_json::from_slice(lines[0]).map_err(|_| pending())?;
    if reservation.schema_version != 1 || reservation.id != id || !reservation.parent.is_absolute()
    {
        return Err(pending());
    }
    let owned = if let Some(line) = lines.get(1) {
        let owned: Owned = serde_json::from_slice(line).map_err(|_| pending())?;
        if owned.schema_version != 1 || owned.id != id {
            return Err(pending());
        }
        Some(owned.identity)
    } else {
        None
    };
    Ok((reservation, owned))
}
fn cleanup_part(directory: &Dir, name: &str, expected: Option<&Identity>) -> std::io::Result<()> {
    let metadata = match directory.symlink_metadata(name) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(pending());
    }
    let Some(expected) = expected else {
        return Err(pending());
    };
    let actual = identity(&directory.open(name)?.metadata()?)?;
    if actual != *expected {
        return Err(pending());
    }
    directory.remove_file(name)
}
pub(super) struct NativePartLease {
    journal: Arc<Dir>,
    directory: Arc<Dir>,
    id: String,
    lease: Option<File>,
    identity: Option<Identity>,
    created: bool,
}
impl NativePartLease {
    pub fn part_name(&self) -> String {
        part_name(&self.id)
    }
    pub fn cleanup(&self) -> std::io::Result<()> {
        if self.created {
            cleanup_part(&self.directory, &self.part_name(), self.identity.as_ref())?;
        }
        match self.journal.remove_file(record_name(&self.id)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
    pub fn verify(&self) -> std::io::Result<()> {
        let Some(expected) = self.identity.as_ref() else {
            return Err(pending());
        };
        let name = self.part_name();
        let metadata = self.directory.symlink_metadata(&name)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(pending());
        }
        if identity(&self.directory.open(name)?.metadata()?)? != *expected {
            return Err(pending());
        }
        Ok(())
    }
}
impl Drop for NativePartLease {
    fn drop(&mut self) {
        let _ = self.cleanup();
        drop(self.lease.take());
    }
}
