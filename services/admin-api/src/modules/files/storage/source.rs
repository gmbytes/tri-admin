use super::{BlobSpec, StorageError};
use crate::config::Config;
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
pub struct VerifiedSource {
    pub(super) file: Option<tokio::fs::File>,
    pub(super) directory: Dir,
    pub(super) name: String,
    pub(super) spec: BlobSpec,
    lease: Option<std::fs::File>,
    lease_name: String,
}
impl Drop for VerifiedSource {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = self.directory.remove_file(&self.name);
        drop(self.lease.take());
        let _ = self.directory.remove_file(&self.lease_name);
    }
}
impl VerifiedSource {
    pub async fn stage<R: AsyncRead + Unpin>(
        config: &Config,
        mut reader: R,
        spec: BlobSpec,
        cancel: CancellationToken,
    ) -> Result<Self, StorageError> {
        let spec = BlobSpec::new(spec.size, &spec.sha256)?;
        let root = config.storage_root.clone();
        let (directory, name, file, lease_name, lease) = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&root).map_err(|_| StorageError::Io)?;
            let root = Dir::open_ambient_dir(root, cap_std::ambient_authority())
                .map_err(|_| StorageError::Io)?;
            root.create_dir_all(".tri-staging")
                .map_err(|_| StorageError::Io)?;
            let directory = root
                .open_dir(".tri-staging")
                .map_err(|_| StorageError::Io)?;
            let id = uuid::Uuid::new_v4();
            let name = format!("{id}.tmp");
            let lease_name = format!("{id}.lease");
            let lease = directory
                .open_with(
                    &lease_name,
                    OpenOptions::new().read(true).write(true).create_new(true),
                )
                .map_err(|_| StorageError::Io)?
                .into_std();
            if lease.try_lock().is_err() {
                drop(lease);
                let _ = directory.remove_file(&lease_name);
                return Err(StorageError::Io);
            }
            let file = match directory.open_with(
                &name,
                OpenOptions::new().read(true).write(true).create_new(true),
            ) {
                Ok(file) => file.into_std(),
                Err(_) => {
                    drop(lease);
                    let _ = directory.remove_file(&lease_name);
                    return Err(StorageError::Io);
                }
            };
            Ok::<_, StorageError>((directory, name, file, lease_name, lease))
        })
        .await
        .map_err(|_| StorageError::Io)??;
        let mut source = Self {
            file: Some(tokio::fs::File::from_std(file)),
            directory,
            name,
            spec,
            lease: Some(lease),
            lease_name,
        };
        let mut buffer = vec![0u8; 64 * 1024];
        let mut hash = Sha256::new();
        let mut count = 0u64;
        loop {
            let length = tokio::select! {biased;_=cancel.cancelled()=>return Err(StorageError::Cancelled),result=reader.read(&mut buffer)=>result.map_err(|_|StorageError::Io)?};
            if length == 0 {
                break;
            }
            count = count
                .checked_add(length as u64)
                .ok_or(StorageError::IntegrityMismatch)?;
            if count > source.spec.size {
                return Err(StorageError::IntegrityMismatch);
            }
            source
                .file
                .as_mut()
                .ok_or(StorageError::Io)?
                .write_all(&buffer[..length])
                .await
                .map_err(|_| StorageError::Io)?;
            hash.update(&buffer[..length]);
        }
        if count != source.spec.size || format!("{:x}", hash.finalize()) != source.spec.sha256 {
            return Err(StorageError::IntegrityMismatch);
        }
        let file = source.file.as_mut().ok_or(StorageError::Io)?;
        file.flush().await.map_err(|_| StorageError::Io)?;
        file.sync_all().await.map_err(|_| StorageError::Io)?;
        file.rewind().await.map_err(|_| StorageError::Io)?;
        if cancel.is_cancelled() {
            return Err(StorageError::Cancelled);
        }
        Ok(source)
    }

    pub async fn sweep(config: &Config) -> Result<StagingSweep, StorageError> {
        let root = config.storage_root.clone();
        tokio::task::spawn_blocking(move || {
            let root = match Dir::open_ambient_dir(root, cap_std::ambient_authority()) {
                Ok(root) => root,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(StagingSweep::default());
                }
                Err(_) => return Err(StorageError::Io),
            };
            let directory = match root.open_dir(".tri-staging") {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(StagingSweep::default());
                }
                Err(_) => return Err(StorageError::Io),
            };
            let mut result = StagingSweep::default();
            let now = std::time::SystemTime::now();
            for entry in directory
                .entries()
                .map_err(|_| StorageError::Io)?
                .take(1000)
            {
                if result.removed >= 100 {
                    break;
                }
                let entry = entry.map_err(|_| StorageError::Io)?;
                result.scanned += 1;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some((stem, extension)) = name.rsplit_once('.') else {
                    continue;
                };
                if !["tmp", "lease"].contains(&extension)
                    || uuid::Uuid::parse_str(stem).is_err()
                    || uuid::Uuid::parse_str(stem)
                        .map(|id| id.to_string() != stem)
                        .unwrap_or(true)
                {
                    continue;
                }
                let metadata = match directory.symlink_metadata(name) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    continue;
                }
                if metadata
                    .modified()
                    .ok()
                    .and_then(|value| now.duration_since(value.into_std()).ok())
                    .is_none_or(|age| age < std::time::Duration::from_secs(3600))
                {
                    continue;
                }
                let data_name = format!("{stem}.tmp");
                let lease_name = format!("{stem}.lease");
                if extension == "lease" && directory.symlink_metadata(&data_name).is_ok() {
                    continue;
                }
                if directory
                    .symlink_metadata(&lease_name)
                    .is_ok_and(|value| !value.is_file() || value.file_type().is_symlink())
                {
                    continue;
                }
                let lease = match directory.open_with(
                    &lease_name,
                    OpenOptions::new().read(true).write(true).create(true),
                ) {
                    Ok(value) => value.into_std(),
                    Err(_) => continue,
                };
                match lease.try_lock() {
                    Ok(()) => {}
                    Err(std::fs::TryLockError::WouldBlock) => {
                        result.active += 1;
                        continue;
                    }
                    Err(_) => continue,
                }
                if extension == "tmp" {
                    let current = match directory.symlink_metadata(&data_name) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    if !current.is_file()
                        || current.file_type().is_symlink()
                        || current
                            .modified()
                            .ok()
                            .and_then(|value| now.duration_since(value.into_std()).ok())
                            .is_none_or(|age| age < std::time::Duration::from_secs(3600))
                    {
                        continue;
                    }
                    if directory.remove_file(&data_name).is_err() {
                        continue;
                    }
                }
                drop(lease);
                let _ = directory.remove_file(&lease_name);
                result.removed += 1;
            }
            Ok(result)
        })
        .await
        .map_err(|_| StorageError::Io)?
    }
}
#[derive(Default, serde::Serialize)]
pub struct StagingSweep {
    pub scanned: u64,
    pub removed: u64,
    pub active: u64,
}
