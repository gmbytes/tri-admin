use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceError {
    #[error("导出工作目录输入无效")]
    Invalid,
    #[error("导出工作目录正在使用")]
    Busy,
    #[error("导出工作目录不存在")]
    Missing,
    #[error("导出工作目录归属未确认")]
    Unconfirmed,
    #[error("导出工作目录读写失败")]
    Io,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Marker {
    version: u32,
    workspace_id: String,
}
fn marker(id: &str) -> Marker {
    Marker {
        version: 1,
        workspace_id: id.into(),
    }
}
fn valid(id: &str) -> Result<(), WorkspaceError> {
    if uuid::Uuid::parse_str(id)
        .ok()
        .is_none_or(|uuid| uuid.to_string() != id)
    {
        return Err(WorkspaceError::Invalid);
    }
    Ok(())
}
fn root(path: &str) -> Result<(Dir, PathBuf), WorkspaceError> {
    std::fs::create_dir_all(path).map_err(|_| WorkspaceError::Io)?;
    let base = Dir::open_ambient_dir(path, cap_std::ambient_authority())
        .map_err(|_| WorkspaceError::Io)?;
    match base.symlink_metadata(".tri-export-work") {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            return Err(WorkspaceError::Unconfirmed);
        }
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => base
            .create_dir_all(".tri-export-work")
            .map_err(|_| WorkspaceError::Io)?,
        Err(_) => return Err(WorkspaceError::Io),
    }
    let directory = base
        .open_dir(".tri-export-work")
        .map_err(|_| WorkspaceError::Io)?;
    let absolute = std::fs::canonicalize(Path::new(path).join(".tri-export-work"))
        .map_err(|_| WorkspaceError::Io)?;
    Ok((directory, absolute))
}
fn check_marker(file: &File, id: &str) -> Result<(), WorkspaceError> {
    let mut bytes = Vec::new();
    file.take(1025)
        .read_to_end(&mut bytes)
        .map_err(|_| WorkspaceError::Io)?;
    if bytes.len() > 1024 {
        return Err(WorkspaceError::Unconfirmed);
    }
    let value: Marker = serde_json::from_slice(&bytes).map_err(|_| WorkspaceError::Unconfirmed)?;
    if value.version != 1 || value.workspace_id != id {
        return Err(WorkspaceError::Unconfirmed);
    }
    Ok(())
}
pub struct Reservation {
    root: Dir,
    path: PathBuf,
    id: String,
    lease: Option<File>,
    registered: bool,
}
impl Reservation {
    /// The flat lease is acquired before DB registration or directory/content
    /// creation, so recovery cannot race an as-yet-unlocked late writer.
    pub fn reserve(storage_root: &str) -> Result<Self, WorkspaceError> {
        let (root, path) = root(storage_root)?;
        let id = uuid::Uuid::new_v4().to_string();
        let mut lease = root
            .open_with(
                format!("{id}.lease"),
                OpenOptions::new().read(true).write(true).create_new(true),
            )
            .map_err(|_| WorkspaceError::Io)?
            .into_std();
        lease.try_lock().map_err(|_| WorkspaceError::Busy)?;
        lease
            .write_all(&serde_json::to_vec(&marker(&id)).map_err(|_| WorkspaceError::Io)?)
            .map_err(|_| WorkspaceError::Io)?;
        lease.sync_all().map_err(|_| WorkspaceError::Io)?;
        Ok(Self {
            root,
            path,
            id,
            lease: Some(lease),
            registered: false,
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Call only after the attempt journal has committed.
    pub fn activate(mut self) -> Result<Workspace, WorkspaceError> {
        self.registered = true;
        self.root
            .create_dir(&self.id)
            .map_err(|_| WorkspaceError::Unconfirmed)?;
        let directory = self
            .root
            .open_dir(&self.id)
            .map_err(|_| WorkspaceError::Io)?;
        let mut owner = directory
            .open_with(
                "owner.json",
                OpenOptions::new().write(true).create_new(true),
            )
            .map_err(|_| WorkspaceError::Io)?;
        owner
            .write_all(&serde_json::to_vec(&marker(&self.id)).map_err(|_| WorkspaceError::Io)?)
            .map_err(|_| WorkspaceError::Io)?;
        owner.sync_all().map_err(|_| WorkspaceError::Io)?;
        Ok(Workspace {
            root: self.root.try_clone().map_err(|_| WorkspaceError::Io)?,
            directory,
            path: self.path.join(&self.id),
            id: self.id.clone(),
            lease: self.lease.take(),
        })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        drop(self.lease.take());
        if !self.registered {
            let _ = self.root.remove_file(format!("{}.lease", self.id));
        }
    }
}
pub struct Workspace {
    root: Dir,
    directory: Dir,
    path: PathBuf,
    id: String,
    lease: Option<File>,
}
impl Workspace {
    pub fn retire(storage_root: &str, id: &str) -> Result<(), WorkspaceError> {
        valid(id)?;
        match Self::open(storage_root, id) {
            Ok(workspace) => return workspace.clean(),
            Err(WorkspaceError::Missing) => (),
            Err(error) => return Err(error),
        }
        let (root, _) = root(storage_root)?;
        if root.symlink_metadata(id).is_ok() {
            return Err(WorkspaceError::Unconfirmed);
        }
        let lease = match root.open_with(
            format!("{id}.lease"),
            OpenOptions::new().read(true).write(true),
        ) {
            Ok(file) => file.into_std(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(WorkspaceError::Io),
        };
        lease.try_lock().map_err(|_| WorkspaceError::Busy)?;
        check_marker(&lease, id)?;
        drop(lease);
        root.remove_file(format!("{id}.lease"))
            .map_err(|_| WorkspaceError::Io)
    }
    pub fn open(storage_root: &str, id: &str) -> Result<Self, WorkspaceError> {
        valid(id)?;
        let (root, path) = root(storage_root)?;
        let lease = root
            .open_with(
                format!("{id}.lease"),
                OpenOptions::new().read(true).write(true),
            )
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    WorkspaceError::Missing
                } else {
                    WorkspaceError::Io
                }
            })?
            .into_std();
        lease.try_lock().map_err(|_| WorkspaceError::Busy)?;
        check_marker(&lease, id)?;
        let meta = root
            .symlink_metadata(id)
            .map_err(|_| WorkspaceError::Missing)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(WorkspaceError::Unconfirmed);
        }
        let directory = root.open_dir(id).map_err(|_| WorkspaceError::Io)?;
        let owner = directory
            .open("owner.json")
            .map_err(|_| WorkspaceError::Unconfirmed)?
            .into_std();
        check_marker(&owner, id)?;
        Ok(Self {
            root,
            directory,
            path: path.join(id),
            id: id.into(),
            lease: Some(lease),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn new_output(&self) -> Result<File, WorkspaceError> {
        self.directory
            .open_with(
                "artifact.part",
                OpenOptions::new().read(true).write(true).create_new(true),
            )
            .map(|file| file.into_std())
            .map_err(|_| WorkspaceError::Io)
    }
    pub fn output(&self) -> Result<File, WorkspaceError> {
        let metadata = self
            .directory
            .symlink_metadata("artifact.part")
            .map_err(|_| WorkspaceError::Missing)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(WorkspaceError::Unconfirmed);
        }
        self.directory
            .open("artifact.part")
            .map(|file| file.into_std())
            .map_err(|_| WorkspaceError::Io)
    }
    pub fn clean(mut self) -> Result<(), WorkspaceError> {
        for entry in self.directory.entries().map_err(|_| WorkspaceError::Io)? {
            let entry = entry.map_err(|_| WorkspaceError::Io)?;
            let name = entry.file_name();
            let metadata = entry.metadata().map_err(|_| WorkspaceError::Io)?;
            if !["artifact.part", "owner.json"]
                .iter()
                .any(|known| name == *known)
                || !metadata.is_file()
                || self
                    .directory
                    .symlink_metadata(&name)
                    .map_err(|_| WorkspaceError::Io)?
                    .file_type()
                    .is_symlink()
            {
                return Err(WorkspaceError::Unconfirmed);
            }
        }
        match self.directory.remove_file("artifact.part") {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(WorkspaceError::Io),
        }
        self.directory
            .remove_file("owner.json")
            .map_err(|_| WorkspaceError::Io)?;
        // Keep the flat lease until the child directory has been removed.
        drop(self.directory);
        self.root
            .remove_dir(&self.id)
            .map_err(|_| WorkspaceError::Io)?;
        drop(self.lease.take());
        self.root
            .remove_file(format!("{}.lease", self.id))
            .map_err(|_| WorkspaceError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_lease_protects_workspace_and_recovery_preserves_unknown_files() {
        let root = tempfile::tempdir().unwrap();
        let reservation = Reservation::reserve(root.path().to_str().unwrap()).unwrap();
        let id = reservation.id().to_owned();
        let workspace = reservation.activate().unwrap();
        assert!(matches!(
            Workspace::open(root.path().to_str().unwrap(), &id),
            Err(WorkspaceError::Busy)
        ));
        let mut file = workspace.new_output().unwrap();
        file.write_all(b"private").unwrap();
        drop(file);
        std::fs::write(workspace.path().join("foreign.note"), b"keep").unwrap();
        let path = workspace.path().to_owned();
        drop(workspace);
        let recovered = Workspace::open(root.path().to_str().unwrap(), &id).unwrap();
        assert_eq!(recovered.clean().unwrap_err(), WorkspaceError::Unconfirmed);
        assert_eq!(
            std::fs::read(path.join("artifact.part")).unwrap(),
            b"private"
        );
        std::fs::remove_file(path.join("foreign.note")).unwrap();
        Workspace::open(root.path().to_str().unwrap(), &id)
            .unwrap()
            .clean()
            .unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn unregistered_reservations_leave_no_lease_and_paths_are_not_accepted_as_ids() {
        let root = tempfile::tempdir().unwrap();
        let reservation = Reservation::reserve(root.path().to_str().unwrap()).unwrap();
        let id = reservation.id().to_owned();
        drop(reservation);
        assert!(
            !root
                .path()
                .join(".tri-export-work")
                .join(format!("{id}.lease"))
                .exists()
        );
        assert!(matches!(
            Workspace::open(root.path().to_str().unwrap(), "../outside"),
            Err(WorkspaceError::Invalid)
        ));
    }
}
