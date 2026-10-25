mod local;
mod s3;
mod source;
use crate::{
    config::Config,
    modules::settings::{
        dto::{Credentials, Integration},
        service,
    },
};
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use mongodb::{Database, bson::oid::ObjectId};
use sha2::{Digest, Sha256};
pub use source::VerifiedSource;
use std::{pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

pub const MAX_BLOB_SIZE: u64 = 5 * 1024 * 1024 * 1024;
pub type BlobStream = Pin<Box<dyn Stream<Item = Result<Bytes, StorageError>> + Send>>;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlobId(ObjectId);
impl BlobId {
    pub fn new() -> Self {
        Self(ObjectId::new())
    }
    pub fn parse(value: &str) -> Result<Self, StorageError> {
        ObjectId::parse_str(value)
            .map(Self)
            .map_err(|_| StorageError::InvalidInput)
    }
}
impl Default for BlobId {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Display for BlobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.to_hex())
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlobSpec {
    size: u64,
    sha256: String,
}
impl BlobSpec {
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn new(size: u64, sha256: &str) -> Result<Self, StorageError> {
        if size > MAX_BLOB_SIZE
            || sha256.len() != 64
            || !sha256.bytes().all(|value| value.is_ascii_hexdigit())
        {
            return Err(StorageError::InvalidInput);
        }
        Ok(Self {
            size,
            sha256: sha256.to_ascii_lowercase(),
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PutResult {
    Created,
    AlreadyPresent,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StorageError {
    #[error("存储输入无效")]
    InvalidInput,
    #[error("文件存储配置不可用")]
    ConfigurationUnavailable,
    #[error("文件存储凭据不可用")]
    SecretUnavailable,
    #[error("存储目标地址被拒绝")]
    DestinationBlocked,
    #[error("文件存储连接失败")]
    ConnectionFailed,
    #[error("文件存储认证被拒绝")]
    AuthenticationRejected,
    #[error("内容不存在")]
    NotFound,
    #[error("内容标识已被不同数据占用")]
    Conflict,
    #[error("内容长度或校验和不匹配")]
    IntegrityMismatch,
    #[error("文件操作已取消")]
    Cancelled,
    #[error("写入结果待确认")]
    OutcomeUnknown,
    #[error("文件存储读写失败")]
    Io,
    #[error("文件存储响应不支持该操作")]
    Unsupported,
}
pub struct BlobStore {
    driver: Driver,
}
enum Driver {
    Local(local::LocalStore),
    S3(Box<s3::S3Store>),
}
impl BlobStore {
    pub async fn local(config: Arc<Config>, subdirectory: &str) -> Result<Self, StorageError> {
        Self::new(
            config,
            Integration::LocalStorage {
                subdirectory: subdirectory.into(),
            },
            None,
        )
        .await
    }
    pub async fn from_profile(
        database: &Database,
        config: Arc<Config>,
        id: ObjectId,
    ) -> Result<Self, StorageError> {
        let (public, secret) = service::storage_definition(database, &config, id)
            .await
            .map_err(|error| match error {
                crate::common::error::AppError::SecretStoreUnavailable => {
                    StorageError::SecretUnavailable
                }
                _ => StorageError::ConfigurationUnavailable,
            })?;
        Self::new(config, public, secret).await
    }
    async fn new(
        config: Arc<Config>,
        public: Integration,
        secret: Option<Credentials>,
    ) -> Result<Self, StorageError> {
        crate::modules::settings::validate::public(&public)
            .map_err(|_| StorageError::InvalidInput)?;
        crate::modules::settings::validate::credentials(&public, secret.as_ref(), true)
            .map_err(|_| StorageError::InvalidInput)?;
        let driver = match public {
            Integration::LocalStorage { subdirectory } => Driver::Local(
                local::LocalStore::open(config.storage_root.clone(), subdirectory).await?,
            ),
            Integration::S3 { .. } => {
                Driver::S3(Box::new(s3::S3Store::new(config, public, secret)?))
            }
            _ => return Err(StorageError::InvalidInput),
        };
        Ok(Self { driver })
    }
    pub async fn put(
        &self,
        id: BlobId,
        source: VerifiedSource,
        cancel: CancellationToken,
    ) -> Result<PutResult, StorageError> {
        if cancel.is_cancelled() {
            return Err(StorageError::Cancelled);
        }
        match &self.driver {
            Driver::Local(store) => store.put(id, source, cancel).await,
            Driver::S3(store) => store.put(id, source, cancel).await,
        }
    }
    pub async fn read(
        &self,
        id: BlobId,
        spec: BlobSpec,
        cancel: CancellationToken,
    ) -> Result<BlobStream, StorageError> {
        if cancel.is_cancelled() {
            return Err(StorageError::Cancelled);
        }
        let stream = match &self.driver {
            Driver::Local(store) => store.read(id, &spec).await?,
            Driver::S3(store) => store.read(id, &spec, cancel.clone()).await?,
        };
        Ok(verified_stream(stream, spec, cancel))
    }
    pub async fn delete(&self, id: BlobId, cancel: CancellationToken) -> Result<(), StorageError> {
        if cancel.is_cancelled() {
            return Err(StorageError::Cancelled);
        }
        match &self.driver {
            Driver::Local(store) => store.delete(id).await,
            Driver::S3(store) => store.delete(id, cancel).await,
        }
    }
}
fn verified_stream(raw: BlobStream, spec: BlobSpec, cancel: CancellationToken) -> BlobStream {
    Box::pin(futures_util::stream::try_unfold(
        (raw, spec, cancel, Sha256::new(), 0u64, None::<Bytes>, false),
        |(mut raw, spec, cancel, mut hash, mut count, mut pending, done)| async move {
            if done {
                return Ok(None);
            }
            loop {
                let next = tokio::select! {biased; _=cancel.cancelled()=>return Err(StorageError::Cancelled),value=raw.next()=>value};
                match next {
                    Some(value) => {
                        let data = value?;
                        count = count
                            .checked_add(data.len() as u64)
                            .ok_or(StorageError::IntegrityMismatch)?;
                        if count > spec.size {
                            return Err(StorageError::IntegrityMismatch);
                        }
                        hash.update(&data);
                        if let Some(previous) = pending.replace(data) {
                            return Ok(Some((
                                previous,
                                (raw, spec, cancel, hash, count, pending, false),
                            )));
                        }
                    }
                    None => {
                        if count != spec.size
                            || format!("{:x}", hash.clone().finalize()) != spec.sha256
                        {
                            return Err(StorageError::IntegrityMismatch);
                        }
                        return Ok(pending
                            .take()
                            .map(|last| (last, (raw, spec, cancel, hash, count, None, true))));
                    }
                }
            }
        },
    ))
}
