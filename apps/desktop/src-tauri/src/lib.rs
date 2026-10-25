use desktop_core::{CredentialVault, DesktopError, SessionManager, Settings};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};
pub mod file_commands;

struct NativeVault;
fn storage_error() -> DesktopError {
    DesktopError {
        code: "SESSION_STORAGE_ERROR",
        message: "无法访问系统会话存储",
    }
}
impl CredentialVault for NativeVault {
    fn load(&self, key: &str) -> Result<Option<String>, DesktopError> {
        match keyring::Entry::new("com.hygd.triadmin", key)
            .map_err(|_| storage_error())?
            .get_password()
        {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(storage_error()),
        }
    }
    fn save(&self, key: &str, token: &str) -> Result<(), DesktopError> {
        keyring::Entry::new("com.hygd.triadmin", key)
            .map_err(|_| storage_error())?
            .set_password(token)
            .map_err(|_| storage_error())
    }
    fn clear(&self, key: &str) -> Result<(), DesktopError> {
        match keyring::Entry::new("com.hygd.triadmin", key)
            .map_err(|_| storage_error())?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(storage_error()),
        }
    }
}
type Shared = Arc<Mutex<SessionManager<NativeVault>>>;
fn internal() -> DesktopError {
    DesktopError {
        code: "SETTINGS_ERROR",
        message: "客户端状态暂不可用",
    }
}

#[tauri::command]
async fn load_settings(state: State<'_, Shared>) -> Result<Settings, DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        Ok(state.lock().map_err(|_| internal())?.settings())
    })
    .await
    .map_err(|_| internal())?
}
#[tauri::command(rename_all = "camelCase")]
async fn set_server(state: State<'_, Shared>, endpoint: String) -> Result<Settings, DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.lock().map_err(|_| internal())?.set_server(&endpoint)
    })
    .await
    .map_err(|_| internal())?
}
#[tauri::command(rename_all = "camelCase")]
async fn load_session(
    state: State<'_, Shared>,
    expected_endpoint: String,
) -> Result<Option<String>, DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state
            .lock()
            .map_err(|_| internal())?
            .load_session(&expected_endpoint)
    })
    .await
    .map_err(|_| internal())?
}
#[tauri::command(rename_all = "camelCase")]
async fn save_session(
    state: State<'_, Shared>,
    expected_endpoint: String,
    refresh_token: String,
) -> Result<(), DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state
            .lock()
            .map_err(|_| internal())?
            .save_session(&expected_endpoint, &refresh_token)
    })
    .await
    .map_err(|_| internal())?
}
#[tauri::command(rename_all = "camelCase")]
async fn clear_session(
    state: State<'_, Shared>,
    expected_endpoint: String,
) -> Result<(), DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state
            .lock()
            .map_err(|_| internal())?
            .clear_session(&expected_endpoint)
    })
    .await
    .map_err(|_| internal())?
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    let result = builder
        .setup(|app| {
            let path = app.path().app_config_dir()?.join("settings.json");
            let defaults = Settings {
                api_base_url: if cfg!(debug_assertions) {
                    "http://127.0.0.1:18088/api/v1".into()
                } else {
                    String::new()
                },
                remember_session: false,
            };
            let manager = SessionManager::open(path, NativeVault, defaults)?;
            app.manage(Arc::new(Mutex::new(manager)));
            app.manage(Arc::new(desktop_core::files::FileHandles::default()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            set_server,
            load_session,
            save_session,
            clear_session,
            file_commands::file_reserve,
            file_commands::file_select_source,
            file_commands::file_select_download,
            file_commands::file_read_chunk,
            file_commands::file_write,
            file_commands::file_complete,
            file_commands::file_cancel,
            file_commands::file_cancel_all
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("客户端启动失败：{error}");
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    #[test]
    fn scoped_windows_session_survives_manager_restart_and_clears() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let endpoint = format!("https://vault-{stamp}.invalid/api/v1");
        let credential = "a".repeat(64);
        let mut manager =
            SessionManager::open(path.clone(), NativeVault, Settings::default()).unwrap();
        manager.set_server(&endpoint).unwrap();
        manager.save_session(&endpoint, &credential).unwrap();
        drop(manager);
        let mut restored = SessionManager::open(path, NativeVault, Settings::default()).unwrap();
        assert_eq!(restored.load_session(&endpoint).unwrap(), Some(credential));
        restored.clear_session(&endpoint).unwrap();
        assert!(restored.load_session(&endpoint).unwrap().is_none());
        let key = format!("native-test-{stamp}");
        NativeVault.save(&key, "fixture-only").unwrap();
        assert_eq!(NativeVault.load(&key).unwrap(), Some("fixture-only".into()));
        NativeVault.clear(&key).unwrap();
        assert!(NativeVault.load(&key).unwrap().is_none());
    }
}
