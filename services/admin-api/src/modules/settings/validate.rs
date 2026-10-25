use super::dto::*;
use crate::common::{error::AppError, validation};
use std::net::IpAddr;
fn bad() -> AppError {
    AppError::Validation("集成配置字段无效".into())
}
pub fn version(value: i64) -> Result<(), AppError> {
    if !(1..i64::MAX).contains(&value) {
        Err(AppError::Validation("版本号无效".into()))
    } else {
        Ok(())
    }
}
pub fn code(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 64
        || !value.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
    {
        Err(bad())
    } else {
        Ok(())
    }
}
pub fn host(value: &str) -> Result<(), AppError> {
    if let Ok(ip) = value.parse::<IpAddr>() {
        return if ip.is_unspecified() || ip.is_multicast() {
            Err(bad())
        } else {
            Ok(())
        };
    }
    if value.is_empty()
        || value.len() > 253
        || !value.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return Err(bad());
    }
    Ok(())
}
fn loopback(value: &str) -> bool {
    value.eq_ignore_ascii_case("localhost")
        || value
            .parse::<IpAddr>()
            .is_ok_and(|value| value.is_loopback())
}
fn database(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 63
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
    {
        Err(bad())
    } else {
        Ok(())
    }
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
            || url.scheme() == "http" && url.host_str().is_some_and(loopback))
    {
        return Err(bad());
    }
    Ok(())
}
pub fn public(value: &Integration) -> Result<(), AppError> {
    match value {
        Integration::MongoDb {
            hosts,
            database: db,
            auth_database,
            replica_set,
            tls: _,
            direct_connection,
        } => {
            if hosts.is_empty() || hosts.len() > 16 || *direct_connection && hosts.len() != 1 {
                return Err(bad());
            }
            let mut unique = std::collections::HashSet::new();
            for item in hosts {
                host(&item.host)?;
                if item.port == 0 || !unique.insert((item.host.to_ascii_lowercase(), item.port)) {
                    return Err(bad());
                }
            }
            database(db)?;
            database(auth_database)?;
            if let Some(name) = replica_set {
                validation::text(name, "副本集名", 128)?;
                if !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                {
                    return Err(bad());
                }
            }
        }
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
        Integration::Smtp {
            host: server,
            port,
            security: _,
            from_address,
        } => {
            host(server)?;
            if *port == 0 || from_address.len() > 254 {
                return Err(bad());
            }
            let (local, domain) = from_address.split_once('@').ok_or_else(bad)?;
            if local.is_empty()
                || local.len() > 64
                || local.starts_with('.')
                || local.ends_with('.')
                || local.contains("..")
                || domain.contains(':')
                || !local
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&c))
            {
                return Err(bad());
            }
            host(domain)?;
        }
        Integration::Http { base_url } => web(base_url)?,
    }
    Ok(())
}
pub fn credentials(
    configuration: &Integration,
    secret: Option<&Credentials>,
    enabled: bool,
) -> Result<(), AppError> {
    if enabled && configuration.kind() == Kind::S3 && secret.is_none() {
        return Err(AppError::Validation("启用对象存储需要配置凭据".into()));
    }
    let Some(secret) = secret else {
        return Ok(());
    };
    if secret.kind() != configuration.kind() {
        return Err(AppError::Validation("凭据类型与集成配置不匹配".into()));
    }
    let text = serde_json::to_string(secret).map_err(|_| bad())?;
    let text = zeroize::Zeroizing::new(text);
    if text.len() > 16384 {
        return Err(bad());
    }
    match secret {
        Credentials::MongoDb { username, password } | Credentials::Smtp { username, password } => {
            if username.is_empty()
                || username.len() > 256
                || password.is_empty()
                || password.len() > 1024
            {
                return Err(bad());
            }
        }
        Credentials::S3 {
            access_key_id,
            secret_access_key,
            session_token,
        } => {
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
        }
        Credentials::Http { header_name, value } => {
            let lower = header_name.to_ascii_lowercase();
            if header_name.len() > 128
                || axum::http::HeaderName::from_bytes(header_name.as_bytes()).is_err()
                || [
                    "host",
                    "content-length",
                    "transfer-encoding",
                    "connection",
                    "te",
                    "trailer",
                    "upgrade",
                    "proxy-authorization",
                    "forwarded",
                ]
                .contains(&lower.as_str())
                || lower.starts_with("x-forwarded-")
                || value.is_empty()
                || value.len() > 8192
                || axum::http::HeaderValue::from_str(value).is_err()
            {
                return Err(bad());
            }
        }
    }
    match configuration {
        Integration::MongoDb {
            hosts, tls: false, ..
        } if !hosts.iter().all(|item| loopback(&item.host)) => {
            return Err(AppError::Validation(
                "非环回 MongoDB 凭据连接需要 TLS".into(),
            ));
        }
        Integration::Smtp {
            host,
            security: MailSecurity::Plain,
            ..
        } if !loopback(host) => {
            return Err(AppError::Validation(
                "明文 SMTP 凭据只允许环回开发地址".into(),
            ));
        }
        _ => {}
    }
    Ok(())
}
