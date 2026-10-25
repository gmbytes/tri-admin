use desktop_core::{CredentialVault, DesktopError, SessionManager, Settings, normalize_endpoint};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Vault(Arc<Mutex<HashMap<String, String>>>);
impl CredentialVault for Vault {
    fn load(&self, key: &str) -> Result<Option<String>, DesktopError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn save(&self, key: &str, value: &str) -> Result<(), DesktopError> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn clear(&self, key: &str) -> Result<(), DesktopError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[test]
fn settings_do_not_contain_credentials_and_session_scope_is_checked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let vault = Vault::default();
    let mut manager =
        SessionManager::open(path.clone(), vault.clone(), Settings::default()).unwrap();
    manager.set_server("https://one.example").unwrap();
    let endpoint = manager.settings().api_base_url;
    manager.save_session(&endpoint, &"a".repeat(64)).unwrap();
    assert_eq!(
        manager.load_session(&endpoint).unwrap(),
        Some("a".repeat(64))
    );
    let settings = std::fs::read_to_string(&path).unwrap();
    assert!(!settings.contains(&"a".repeat(64)));
    assert!(!settings.contains("refreshToken"));
    assert!(
        manager
            .save_session("https://two.example/api/v1", &"b".repeat(64))
            .is_err()
    );
    manager.set_server("https://two.example").unwrap();
    assert!(manager.load_session("https://one.example/api/v1").is_err());
    assert!(vault.0.lock().unwrap().is_empty());
    let restored = SessionManager::open(path, vault, Settings::default()).unwrap();
    assert_eq!(
        restored.settings().api_base_url,
        "https://two.example/api/v1"
    );
    assert!(!restored.settings().remember_session);
}

#[test]
fn logout_disables_restore_and_clears_the_scoped_secret() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let vault = Vault::default();
    let mut manager =
        SessionManager::open(path.clone(), vault.clone(), Settings::default()).unwrap();
    manager.set_server("http://localhost:18088").unwrap();
    let endpoint = manager.settings().api_base_url;
    manager.save_session(&endpoint, &"a".repeat(64)).unwrap();
    assert!(manager.settings().remember_session);
    manager.clear_session(&endpoint).unwrap();
    assert!(manager.load_session(&endpoint).unwrap().is_none());
    assert!(
        !SessionManager::open(path, vault, Settings::default())
            .unwrap()
            .settings()
            .remember_session
    );
}

#[test]
fn endpoint_validation_matches_the_client_security_boundary() {
    assert_eq!(
        normalize_endpoint("https://EXAMPLE.com:443/").unwrap(),
        "https://example.com/api/v1"
    );
    assert_eq!(
        normalize_endpoint("http://[::1]:18088").unwrap(),
        "http://[::1]:18088/api/v1"
    );
    for value in [
        "http://example.com",
        "https://user:secret@example.com",
        "https://example.com?secret=value",
        "https://example.com/old",
        "http://localhost:0",
    ] {
        assert!(normalize_endpoint(value).is_err());
    }
}
