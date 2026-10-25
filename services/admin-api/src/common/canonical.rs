use super::error::AppError;
use serde::Serialize;
use sha2::{Digest, Sha256};
pub fn json<T: Serialize>(value: &T) -> Result<String, AppError> {
    let mut value = serde_json::to_value(value).map_err(|_| AppError::Internal)?;
    value.sort_all_objects();
    serde_json::to_string(&value).map_err(|_| AppError::Internal)
}
pub fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
