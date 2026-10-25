//! Platform-independent desktop settings and scoped session persistence.

pub mod files;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, fs, path::PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub api_base_url: String,
    pub remember_session: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DesktopError {
    pub code: &'static str,
    pub message: &'static str,
}
impl fmt::Display for DesktopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for DesktopError {}
fn error(code: &'static str, message: &'static str) -> DesktopError {
    DesktopError { code, message }
}

pub fn normalize_endpoint(raw: &str) -> Result<String, DesktopError> {
    let parsed = url::Url::parse(raw.trim())
        .map_err(|_| error("INVALID_ENDPOINT", "请输入完整服务器地址"))?;
    let local = matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
        return Err(error(
            "INVALID_ENDPOINT",
            "服务器需要 HTTPS，本机开发可以使用 HTTP",
        ));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.port() == Some(0)
    {
        return Err(error(
            "INVALID_ENDPOINT",
            "服务器地址不能包含凭据、查询、片段或无效端口",
        ));
    }
    if !matches!(parsed.path().trim_end_matches('/'), "" | "/api/v1") {
        return Err(error(
            "INVALID_ENDPOINT",
            "请填写服务器根地址或 /api/v1 地址",
        ));
    }
    Ok(format!("{}/api/v1", parsed.origin().ascii_serialization()))
}

pub trait CredentialVault: Send {
    fn load(&self, key: &str) -> Result<Option<String>, DesktopError>;
    fn save(&self, key: &str, token: &str) -> Result<(), DesktopError>;
    fn clear(&self, key: &str) -> Result<(), DesktopError>;
}

fn account(endpoint: &str) -> String {
    format!("session-{:x}", Sha256::digest(endpoint.as_bytes()))
}

pub struct SessionManager<V: CredentialVault> {
    path: PathBuf,
    vault: V,
    settings: Settings,
}
impl<V: CredentialVault> SessionManager<V> {
    pub fn open(path: PathBuf, vault: V, defaults: Settings) -> Result<Self, DesktopError> {
        let mut settings = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Settings>(&bytes)
                .map_err(|_| error("SETTINGS_ERROR", "客户端配置格式无效"))?,
            Err(read) if read.kind() == std::io::ErrorKind::NotFound => defaults,
            Err(_) => return Err(error("SETTINGS_ERROR", "无法读取客户端配置")),
        };
        if !settings.api_base_url.is_empty() {
            settings.api_base_url = normalize_endpoint(&settings.api_base_url)?;
        }
        Ok(Self {
            path,
            vault,
            settings,
        })
    }
    pub fn settings(&self) -> Settings {
        self.settings.clone()
    }
    fn persist(&self, settings: &Settings) -> Result<(), DesktopError> {
        let parent = self
            .path
            .parent()
            .ok_or(error("SETTINGS_ERROR", "客户端配置位置无效"))?;
        fs::create_dir_all(parent)
            .map_err(|_| error("SETTINGS_ERROR", "无法创建客户端配置目录"))?;
        let temporary = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(settings)
            .map_err(|_| error("SETTINGS_ERROR", "无法保存客户端配置"))?;
        fs::write(&temporary, bytes).map_err(|_| error("SETTINGS_ERROR", "无法写入客户端配置"))?;
        fs::rename(&temporary, &self.path)
            .map_err(|_| error("SETTINGS_ERROR", "无法更新客户端配置"))?;
        Ok(())
    }
    fn check(&self, endpoint: &str) -> Result<(), DesktopError> {
        if normalize_endpoint(endpoint)? != self.settings.api_base_url {
            return Err(error("SESSION_CHANGED", "服务器已切换，请重新登录"));
        }
        Ok(())
    }
    pub fn set_server(&mut self, endpoint: &str) -> Result<Settings, DesktopError> {
        let endpoint = normalize_endpoint(endpoint)?;
        if endpoint == self.settings.api_base_url {
            return Ok(self.settings());
        }
        let previous = self.settings.clone();
        let next = Settings {
            api_base_url: endpoint,
            remember_session: false,
        };
        self.persist(&next)?;
        self.settings = next;
        if previous.remember_session {
            let _ = self.vault.clear(&account(&previous.api_base_url));
        }
        Ok(self.settings())
    }
    pub fn load_session(&self, endpoint: &str) -> Result<Option<String>, DesktopError> {
        self.check(endpoint)?;
        if !self.settings.remember_session {
            return Ok(None);
        }
        self.vault.load(&account(endpoint))
    }
    pub fn save_session(&mut self, endpoint: &str, token: &str) -> Result<(), DesktopError> {
        self.check(endpoint)?;
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(error("INVALID_SESSION", "会话凭证格式无效"));
        }
        let key = account(endpoint);
        self.vault.save(&key, token)?;
        let next = Settings {
            api_base_url: self.settings.api_base_url.clone(),
            remember_session: true,
        };
        if let Err(failure) = self.persist(&next) {
            let _ = self.vault.clear(&key);
            return Err(failure);
        }
        self.settings = next;
        Ok(())
    }
    pub fn clear_session(&mut self, endpoint: &str) -> Result<(), DesktopError> {
        self.check(endpoint)?;
        let persisted = self.settings.remember_session;
        self.settings.remember_session = false;
        let file = self.persist(&self.settings);
        let vault = if persisted {
            self.vault.clear(&account(endpoint))
        } else {
            Ok(())
        };
        file?;
        vault
    }
}
