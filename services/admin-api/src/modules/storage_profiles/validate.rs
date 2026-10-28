use super::dto::*;
use crate::common::{error::AppError, validation};
fn bad() -> AppError {
    AppError::Validation("文件存储配置无效".into())
}
fn relative(value: &str, local: bool) -> Result<(), AppError> {
    if value.len() > 512 || value.contains('\\') || value.starts_with('/') || value.ends_with('/') {
        return Err(bad());
    }
    for part in value.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || !part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err(bad());
        }
        if local {
            let name = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            if ["CON", "PRN", "AUX", "NUL"].contains(&name.as_str())
                || name.len() == 4
                    && (name.starts_with("COM") || name.starts_with("LPT"))
                    && matches!(name.as_bytes()[3], b'1'..=b'9')
            {
                return Err(bad());
            }
        }
    }
    Ok(())
}
fn web(value: &str) -> Result<(), AppError> {
    if value.len() > 2048 {
        return Err(bad());
    }
    let url = url::Url::parse(value).map_err(|_| bad())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || !(url.scheme() == "https"
            || url.scheme() == "http"
                && match url.host() {
                    Some(url::Host::Domain(value)) => value.eq_ignore_ascii_case("localhost"),
                    Some(url::Host::Ipv4(value)) => value.is_loopback(),
                    Some(url::Host::Ipv6(value)) => value.is_loopback(),
                    None => false,
                })
    {
        return Err(bad());
    }
    Ok(())
}
pub fn public(value: &Integration) -> Result<(), AppError> {
    match value {
        Integration::LocalStorage { subdirectory } => relative(subdirectory, true)?,
        Integration::S3 {
            endpoint,
            bucket,
            region,
            prefix,
            ..
        } => {
            web(endpoint)?;
            if !(3..=63).contains(&bucket.len())
                || bucket.contains("..")
                || !bucket.starts_with(|c: char| c.is_ascii_alphanumeric())
                || !bucket.ends_with(|c: char| c.is_ascii_alphanumeric())
                || !bucket
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".-".contains(&c))
                || bucket.parse::<std::net::Ipv4Addr>().is_ok()
            {
                return Err(bad());
            }
            validation::text(region, "区域", 64)?;
            if !region
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            {
                return Err(bad());
            }
            if !prefix.is_empty() {
                relative(prefix, false)?;
            }
        }
    }
    Ok(())
}
pub fn credentials(
    configuration: &Integration,
    secret: Option<&Credentials>,
    enabled: bool,
) -> Result<(), AppError> {
    if enabled && configuration.kind() == Kind::S3 && secret.is_none() {
        return Err(AppError::Validation("对象存储凭据未配置".into()));
    }
    let Some(secret) = secret else { return Ok(()) };
    if secret.kind() != configuration.kind() {
        return Err(bad());
    }
    let text = zeroize::Zeroizing::new(serde_json::to_string(secret).map_err(|_| bad())?);
    if text.len() > 16384 {
        return Err(bad());
    }
    let Credentials::S3 {
        access_key_id,
        secret_access_key,
        session_token,
    } = secret;
    if access_key_id.is_empty()
        || access_key_id.len() > 256
        || secret_access_key.is_empty()
        || secret_access_key.len() > 1024
        || session_token
            .as_ref()
            .is_some_and(|value| value.len() > 8192)
    {
        return Err(bad());
    }
    Ok(())
}
