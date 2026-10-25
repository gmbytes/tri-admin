use axum::http::HeaderValue;
use std::collections::BTreeMap;
use std::{fmt, net::SocketAddr};
use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_name: String,
    pub cors_origins: Vec<HeaderValue>,
    pub login_limit_per_ip: i64,
    mongodb_uri: Option<String>,
    jwt_signing_key: String,
    settings_keys: BTreeMap<String, Zeroizing<[u8; 32]>>,
    settings_active_key: Option<String>,
    pub storage_root: String,
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
                if origin == "tauri://localhost" {
                    return Ok(HeaderValue::from_static("tauri://localhost"));
                }
                let value = url::Url::parse(origin)
                    .map_err(|_| ConfigError("TRI_CORS_ORIGINS 必须为明确来源"))?;
                if !["http", "https"].contains(&value.scheme())
                    || value.host_str().is_none()
                    || !value.username().is_empty()
                    || value.password().is_some()
                    || value.query().is_some()
                    || value.fragment().is_some()
                    || value.path() != "/"
                {
                    return Err(ConfigError("TRI_CORS_ORIGINS 必须为无凭据的明确来源"));
                }
                HeaderValue::from_str(&value.origin().ascii_serialization())
                    .map_err(|_| ConfigError("TRI_CORS_ORIGINS 格式无效"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let login_limit_per_ip = lookup("TRI_LOGIN_LIMIT_PER_IP")
            .unwrap_or_else(|| "100".into())
            .parse::<i64>()
            .map_err(|_| ConfigError("TRI_LOGIN_LIMIT_PER_IP 必须为 1–10000"))?;
        if !(1..=10000).contains(&login_limit_per_ip) {
            return Err(ConfigError("TRI_LOGIN_LIMIT_PER_IP 必须为 1–10000"));
        }
        let mut settings_keys = BTreeMap::new();
        if let Some(raw) = lookup("TRI_SETTINGS_KEYS").map(Zeroizing::new) {
            if raw.is_empty() || raw.split(',').count() > 8 {
                return Err(ConfigError("配置密钥环需包含1–8项"));
            }
            for item in raw.split(',') {
                let (id, hex) = item
                    .split_once(':')
                    .ok_or(ConfigError("配置密钥环格式无效"))?;
                if id.is_empty()
                    || id.len() > 32
                    || !id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                    || hex.len() != 64
                    || !hex.bytes().all(|c| c.is_ascii_hexdigit())
                    || settings_keys.contains_key(id)
                {
                    return Err(ConfigError("配置密钥标识或长度无效"));
                }
                let mut bytes = Zeroizing::new([0u8; 32]);
                for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
                    bytes[index] = u8::from_str_radix(
                        std::str::from_utf8(pair).map_err(|_| ConfigError("配置密钥格式无效"))?,
                        16,
                    )
                    .map_err(|_| ConfigError("配置密钥格式无效"))?;
                }
                settings_keys.insert(id.to_owned(), bytes);
            }
        }
        let settings_active_key = lookup("TRI_SETTINGS_ACTIVE_KEY");
        if settings_active_key
            .as_ref()
            .is_some_and(|id| !settings_keys.contains_key(id))
            || !settings_keys.is_empty() && settings_active_key.is_none()
        {
            return Err(ConfigError("活动配置密钥不在密钥环中"));
        }
        let storage_root = lookup("TRI_STORAGE_ROOT").unwrap_or_else(|| "data/storage".into());
        if storage_root.is_empty() || storage_root.len() > 1024 || storage_root.contains('\0') {
            return Err(ConfigError("TRI_STORAGE_ROOT 无效"));
        }
        Ok(Self {
            bind,
            database_name,
            cors_origins,
            login_limit_per_ip,
            mongodb_uri,
            jwt_signing_key,
            settings_keys,
            settings_active_key,
            storage_root,
        })
    }

    pub fn mongodb_uri(&self) -> Option<&str> {
        self.mongodb_uri.as_deref()
    }
    pub fn jwt_signing_key(&self) -> &str {
        &self.jwt_signing_key
    }
    pub fn settings_key_ids(&self) -> Vec<String> {
        self.settings_keys.keys().cloned().collect()
    }
    pub fn settings_active_key(&self) -> Option<&str> {
        self.settings_active_key.as_deref()
    }
    pub(crate) fn settings_key(&self, id: &str) -> Option<&[u8; 32]> {
        self.settings_keys.get(id).map(|value| &**value)
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
            .field("settings_key_ids", &self.settings_key_ids())
            .finish()
    }
}
