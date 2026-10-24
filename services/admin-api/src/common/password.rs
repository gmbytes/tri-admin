use crate::common::error::AppError;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use std::sync::LazyLock;

static DUMMY: LazyLock<Result<String, ()>> = LazyLock::new(|| {
    let salt = SaltString::encode_b64(b"tri-admin-dummy-salt").map_err(|_| ())?;
    Argon2::default()
        .hash_password(b"non-account-placeholder", &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| ())
});

pub fn dummy_hash() -> Result<&'static str, AppError> {
    DUMMY
        .as_ref()
        .map(String::as_str)
        .map_err(|_| AppError::Internal)
}

pub async fn verify(password: String, hash: String) -> Result<bool, AppError> {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        })
    })
    .await
    .map_err(|_| AppError::Internal)
}
