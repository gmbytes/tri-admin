use crate::{common::error::AppError, config::Config};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SealedBox {
    pub format: String,
    pub key_id: String,
    pub nonce: String,
    pub ciphertext: String,
}
fn context(aad: &[u8], key_id: &str) -> Vec<u8> {
    let mut value = b"tri-admin/vault/xchacha20poly1305/v1\0".to_vec();
    value.extend_from_slice(key_id.as_bytes());
    value.push(0);
    value.extend_from_slice(aad);
    value
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn decode(raw: &str) -> Result<Vec<u8>, AppError> {
    if !raw.len().is_multiple_of(2)
        || raw.len() > 65536
        || !raw.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(AppError::SecretStoreUnavailable);
    }
    raw.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(
                std::str::from_utf8(pair).map_err(|_| AppError::SecretStoreUnavailable)?,
                16,
            )
            .map_err(|_| AppError::SecretStoreUnavailable)
        })
        .collect()
}
pub fn seal(config: &Config, aad: &[u8], plaintext: &[u8]) -> Result<SealedBox, AppError> {
    let id = config
        .settings_active_key()
        .ok_or(AppError::SecretStoreUnavailable)?;
    let key = config
        .settings_key(id)
        .ok_or(AppError::SecretStoreUnavailable)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| AppError::SecretStoreUnavailable)?;
    let mut nonce = [0u8; 24];
    OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| AppError::Internal)?;
    let iv: XNonce = nonce.into();
    let context = context(aad, id);
    let ciphertext = cipher
        .encrypt(
            &iv,
            Payload {
                msg: plaintext,
                aad: &context,
            },
        )
        .map_err(|_| AppError::SecretStoreUnavailable)?;
    Ok(SealedBox {
        format: "xchacha20poly1305/v1".into(),
        key_id: id.into(),
        nonce: hex(&nonce),
        ciphertext: hex(&ciphertext),
    })
}
pub fn open(
    config: &Config,
    aad: &[u8],
    sealed: &SealedBox,
) -> Result<Zeroizing<Vec<u8>>, AppError> {
    if sealed.format != "xchacha20poly1305/v1" {
        return Err(AppError::SecretStoreUnavailable);
    }
    let key = config
        .settings_key(&sealed.key_id)
        .ok_or(AppError::SecretStoreUnavailable)?;
    let nonce: [u8; 24] = decode(&sealed.nonce)?
        .try_into()
        .map_err(|_| AppError::SecretStoreUnavailable)?;
    let iv: XNonce = nonce.into();
    let ciphertext = decode(&sealed.ciphertext)?;
    let context = context(aad, &sealed.key_id);
    let cipher =
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| AppError::SecretStoreUnavailable)?;
    cipher
        .decrypt(
            &iv,
            Payload {
                msg: &ciphertext,
                aad: &context,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| AppError::SecretStoreUnavailable)
}
