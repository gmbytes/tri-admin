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
}
impl Drop for VerifiedSource {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = self.directory.remove_file(&self.name);
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
        let (directory, name, file) = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&root).map_err(|_| StorageError::Io)?;
            let root = Dir::open_ambient_dir(root, cap_std::ambient_authority())
                .map_err(|_| StorageError::Io)?;
            root.create_dir_all(".tri-staging")
                .map_err(|_| StorageError::Io)?;
            let directory = root
                .open_dir(".tri-staging")
                .map_err(|_| StorageError::Io)?;
            let name = format!("{}.tmp", uuid::Uuid::new_v4());
            let file = directory
                .open_with(
                    &name,
                    OpenOptions::new().read(true).write(true).create_new(true),
                )
                .map_err(|_| StorageError::Io)?
                .into_std();
            Ok::<_, StorageError>((directory, name, file))
        })
        .await
        .map_err(|_| StorageError::Io)??;
        let mut source = Self {
            file: Some(tokio::fs::File::from_std(file)),
            directory,
            name,
            spec,
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
}
