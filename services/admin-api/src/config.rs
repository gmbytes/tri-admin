use axum::http::HeaderValue;
use std::{fmt, net::SocketAddr};
use thiserror::Error;

#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_name: String,
    pub cors_origins: Vec<HeaderValue>,
    mongodb_uri: Option<String>,
    jwt_signing_key: String,
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ConfigError(pub &'static str);

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let _ = dotenvy::dotenv();
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let jwt_signing_key =
            lookup("TRI_JWT_SIGNING_KEY").ok_or(ConfigError("请设置 TRI_JWT_SIGNING_KEY"))?;
        if jwt_signing_key.len() < 32 {
            return Err(ConfigError("TRI_JWT_SIGNING_KEY 至少需要 32 字节"));
        }
        let mongodb_uri = lookup("TRI_MONGODB_URI").filter(|value| !value.trim().is_empty());
        if mongodb_uri
            .as_ref()
            .is_some_and(|uri| !uri.starts_with("mongodb://") && !uri.starts_with("mongodb+srv://"))
        {
            return Err(ConfigError("TRI_MONGODB_URI 格式无效"));
        }
        let database_name = lookup("TRI_MONGODB_DATABASE").unwrap_or_else(|| "tri_admin".into());
        if database_name.is_empty()
            || database_name.len() > 63
            || !database_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ConfigError("TRI_MONGODB_DATABASE 名称无效"));
        }
        let bind = lookup("TRI_ADMIN_BIND")
            .unwrap_or_else(|| "127.0.0.1:18088".into())
            .parse()
            .map_err(|_| ConfigError("TRI_ADMIN_BIND 地址无效"))?;
        let origins = lookup("TRI_CORS_ORIGINS").unwrap_or_else(|| {
            "http://localhost:1420,http://tauri.localhost,tauri://localhost".into()
        });
        let cors_origins = origins
            .split(',')
            .map(str::trim)
            .map(|origin| {
                if origin == "*"
                    || !(origin.starts_with("http://")
                        || origin.starts_with("https://")
                        || origin == "tauri://localhost")
                {
                    return Err(ConfigError("TRI_CORS_ORIGINS 必须为明确来源"));
                }
                HeaderValue::from_str(origin).map_err(|_| ConfigError("TRI_CORS_ORIGINS 格式无效"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            bind,
            database_name,
            cors_origins,
            mongodb_uri,
            jwt_signing_key,
        })
    }

    pub fn mongodb_uri(&self) -> Option<&str> {
        self.mongodb_uri.as_deref()
    }
    pub fn jwt_signing_key(&self) -> &str {
        &self.jwt_signing_key
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("database_name", &self.database_name)
            .field(
                "mongodb_uri",
                &self.mongodb_uri.as_ref().map(|_| "<redacted>"),
            )
            .field("jwt_signing_key", &"<redacted>")
            .finish()
    }
}
