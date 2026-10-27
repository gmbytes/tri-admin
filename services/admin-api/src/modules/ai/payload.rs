#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> crate::config::Config {
        crate::config::Config::from_lookup(|name| match name {
            "TRI_JWT_SIGNING_KEY" => Some("fixture-signing-key-at-least-32-bytes-long".into()),
            "TRI_SETTINGS_KEYS" => Some(format!("fixture:{}", "11".repeat(32))),
            "TRI_SETTINGS_ACTIVE_KEY" => Some("fixture".into()),
            _ => None,
        })
        .unwrap()
    }
    #[test]
    fn private_payload_exceeds_one_vault_chunk_but_remains_bounded_and_identity_bound() {
        let config = config();
        let private = vec![b'x'; 1024 * 1024 + 17];
        let blob = seal(&config, "run/owner/input-hash/state", &private, MAX_STATE).unwrap();
        assert!(blob.parts.len() > 1);
        assert_eq!(
            &*open(&config, "run/owner/input-hash/state", &blob, MAX_STATE).unwrap(),
            &private
        );
        assert!(open(&config, "other-run/state", &blob, MAX_STATE).is_err());
        assert!(open(&config, "run/owner/input-hash/state", &blob, 512 * 1024).is_err());
        let mut reordered = blob.clone();
        reordered.parts.swap(0, 1);
        assert!(open(&config, "run/owner/input-hash/state", &reordered, MAX_STATE).is_err());
        let mut truncated = blob.clone();
        truncated.parts.pop();
        assert!(open(&config, "run/owner/input-hash/state", &truncated, MAX_STATE).is_err());
    }
    #[test]
    fn payload_metadata_and_missing_keys_cannot_bypass_authenticated_limits() {
        let config = config();
        let mut blob = seal(&config, "run/input", b"PRIVATE-PAYLOAD", MAX_INPUT).unwrap();
        let missing = crate::config::Config::from_lookup(|name| {
            if name == "TRI_JWT_SIGNING_KEY" {
                Some("fixture-signing-key-at-least-32-bytes-long".into())
            } else {
                None
            }
        })
        .unwrap();
        assert!(open(&missing, "run/input", &blob, MAX_INPUT).is_err());
        blob.bytes += 1;
        assert!(open(&config, "run/input", &blob, MAX_INPUT).is_err());
        assert!(seal(&config, "run/input", &vec![0; MAX_INPUT + 1], MAX_INPUT).is_err());
        let empty = seal(&config, "run/state", b"", MAX_STATE).unwrap();
        assert!(
            open(&config, "run/state", &empty, MAX_STATE)
                .unwrap()
                .is_empty()
        );
    }
}
use crate::{
    common::{
        error::AppError,
        vault::{self, SealedBox},
    },
    config::Config,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
const CHUNK: usize = 256 * 1024;
pub const MAX_INPUT: usize = 512 * 1024;
pub const MAX_STATE: usize = 7 * 1024 * 1024;
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Blob {
    format: String,
    bytes: usize,
    parts: Vec<SealedBox>,
}
fn aad(base: &str, bytes: usize, count: usize, index: usize) -> String {
    format!("tri-admin/ai/chunks1/{base}/{bytes}/{count}/{index}")
}
pub fn seal(config: &Config, base: &str, plain: &[u8], maximum: usize) -> Result<Blob, AppError> {
    if maximum > MAX_STATE || plain.len() > maximum {
        return Err(AppError::PayloadTooLarge);
    }
    let count = plain.len().div_ceil(CHUNK).max(1);
    let mut parts = Vec::with_capacity(count);
    if plain.is_empty() {
        parts.push(vault::seal(config, aad(base, 0, 1, 0).as_bytes(), plain)?);
    } else {
        for (index, chunk) in plain.chunks(CHUNK).enumerate() {
            parts.push(vault::seal(
                config,
                aad(base, plain.len(), count, index).as_bytes(),
                chunk,
            )?);
        }
    }
    Ok(Blob {
        format: "chunks1".into(),
        bytes: plain.len(),
        parts,
    })
}
pub fn open(
    config: &Config,
    base: &str,
    blob: &Blob,
    maximum: usize,
) -> Result<Zeroizing<Vec<u8>>, AppError> {
    if maximum > MAX_STATE
        || blob.format != "chunks1"
        || blob.bytes > maximum
        || blob.parts.len() != blob.bytes.div_ceil(CHUNK).max(1)
    {
        return Err(AppError::SecretStoreUnavailable);
    }
    let mut output = Zeroizing::new(Vec::with_capacity(blob.bytes));
    for (index, part) in blob.parts.iter().enumerate() {
        let bytes = vault::open_bounded(
            config,
            aad(base, blob.bytes, blob.parts.len(), index).as_bytes(),
            part,
            CHUNK,
        )?;
        let expected = if index + 1 == blob.parts.len() {
            blob.bytes.saturating_sub(index * CHUNK)
        } else {
            CHUNK
        };
        if bytes.len() != expected {
            return Err(AppError::SecretStoreUnavailable);
        }
        output.extend_from_slice(&bytes);
    }
    Ok(output)
}
