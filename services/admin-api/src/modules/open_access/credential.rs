use crate::common::error::AppError;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use mongodb::bson::oid::ObjectId;
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub fn hash(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}
pub fn issue(kind: &str, id: ObjectId) -> Result<(String, String), AppError> {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| AppError::Internal)?;
    let raw = format!("{kind}.{}.{}", id.to_hex(), URL_SAFE_NO_PAD.encode(bytes));
    let digest = hash(&raw);
    Ok((raw, digest))
}
pub fn parse(kind: &str, raw: &str) -> Option<ObjectId> {
    if raw.len() != kind.len() + 1 + 24 + 1 + 43 {
        return None;
    }
    let mut parts = raw.split('.');
    if parts.next()? != kind {
        return None;
    }
    let text = parts.next()?;
    let id = ObjectId::parse_str(text).ok()?;
    if text != id.to_hex() {
        return None;
    }
    let secret = parts.next()?;
    let bytes = URL_SAFE_NO_PAD.decode(secret).ok()?;
    if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(&bytes) != secret || parts.next().is_some() {
        return None;
    }
    Some(id)
}
pub fn verify(raw: &str, digest: &str) -> bool {
    digest.len() == 64 && bool::from(hash(raw).as_bytes().ct_eq(digest.as_bytes()))
}
