use super::{BlobId, BlobSpec, BlobStream, PutResult, StorageError, VerifiedSource};
use cap_std::fs::Dir;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{io::Read, sync::Arc};
use tokio_util::{io::ReaderStream, sync::CancellationToken};
pub(super) struct LocalStore {
    directory: Arc<Dir>,
}
fn io(error: std::io::Error) -> StorageError {
    if error.kind() == std::io::ErrorKind::NotFound {
        StorageError::NotFound
    } else {
        StorageError::Io
    }
}
impl LocalStore {
    pub async fn open(root: String, subdirectory: String) -> Result<Self, StorageError> {
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&root).map_err(io)?;
            let root = Dir::open_ambient_dir(root, cap_std::ambient_authority()).map_err(io)?;
            root.create_dir_all(&subdirectory).map_err(io)?;
            let base = root.open_dir(&subdirectory).map_err(io)?;
            base.create_dir_all("objects").map_err(io)?;
            Ok(Self {
                directory: Arc::new(base.open_dir("objects").map_err(io)?),
            })
        })
        .await
        .map_err(|_| StorageError::Io)?
    }
    pub async fn put(
        &self,
        id: BlobId,
        source: VerifiedSource,
        cancel: CancellationToken,
    ) -> Result<PutResult, StorageError> {
        let destination = self.directory.clone();
        tokio::task::spawn_blocking(move || {
            if cancel.is_cancelled() {
                return Err(StorageError::Cancelled);
            }
            match source
                .directory
                .hard_link(&source.name, &destination, id.to_string())
            {
                Ok(()) => Ok(PutResult::Created),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let mut file = destination.open(id.to_string()).map_err(io)?;
                    if file.metadata().map_err(io)?.len() != source.spec.size {
                        return Err(StorageError::Conflict);
                    }
                    let mut hash = Sha256::new();
                    let mut buffer = [0u8; 64 * 1024];
                    loop {
                        if cancel.is_cancelled() {
                            return Err(StorageError::Cancelled);
                        }
                        let length = file.read(&mut buffer).map_err(io)?;
                        if length == 0 {
                            break;
                        }
                        hash.update(&buffer[..length]);
                    }
                    if format!("{:x}", hash.finalize()) != source.spec.sha256 {
                        return Err(StorageError::Conflict);
                    }
                    Ok(PutResult::AlreadyPresent)
                }
                Err(error) => Err(io(error)),
            }
        })
        .await
        .map_err(|_| StorageError::Io)?
    }
    pub async fn read(&self, id: BlobId, spec: &BlobSpec) -> Result<BlobStream, StorageError> {
        let directory = self.directory.clone();
        let size = spec.size;
        let file = tokio::task::spawn_blocking(move || {
            let file = directory.open(id.to_string()).map_err(io)?;
            if file.metadata().map_err(io)?.len() != size {
                return Err(StorageError::IntegrityMismatch);
            }
            Ok(file.into_std())
        })
        .await
        .map_err(|_| StorageError::Io)??;
        Ok(Box::pin(
            ReaderStream::with_capacity(tokio::fs::File::from_std(file), 64 * 1024)
                .map(|value| value.map_err(|_| StorageError::Io)),
        ))
    }
    pub async fn delete(&self, id: BlobId) -> Result<(), StorageError> {
        let directory = self.directory.clone();
        tokio::task::spawn_blocking(move || match directory.remove_file(id.to_string()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io(error)),
        })
        .await
        .map_err(|_| StorageError::Io)?
    }
}
