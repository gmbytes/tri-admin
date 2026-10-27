use mongodb::bson::oid::ObjectId;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{RwLock, watch};
use tokio_util::sync::CancellationToken;
use tri_mcp_runtime::process::Client;
pub struct Registry {
    pub service_id: String,
    entries: Mutex<BTreeMap<ObjectId, Arc<Entry>>>,
    pub shutdown: CancellationToken,
}
pub struct Entry {
    pub connection_id: ObjectId,
    pub stop: CancellationToken,
    pub client: RwLock<Option<Arc<Client>>>,
    done: watch::Sender<bool>,
    pub calls: Mutex<BTreeMap<ObjectId, CancellationToken>>,
}
impl Entry {
    fn new(connection_id: ObjectId) -> Self {
        let (done, _) = watch::channel(false);
        Self {
            connection_id,
            stop: CancellationToken::new(),
            client: RwLock::new(None),
            done,
            calls: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn finished(&self) {
        self.done.send_replace(true);
    }
    pub async fn wait(&self) -> bool {
        let mut done = self.done.subscribe();
        loop {
            if *done.borrow() {
                return true;
            }
            if done.changed().await.is_err() {
                return false;
            }
        }
    }
    pub fn cancel_call(&self, id: ObjectId) {
        if let Some(token) = self
            .calls
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .get(&id)
        {
            token.cancel();
        }
    }
}
impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
impl Registry {
    pub fn new() -> Self {
        Self {
            service_id: uuid::Uuid::new_v4().to_string(),
            entries: Mutex::new(BTreeMap::new()),
            shutdown: CancellationToken::new(),
        }
    }
    pub fn insert(&self, id: ObjectId, connection_id: ObjectId) -> Option<Arc<Entry>> {
        let mut entries = self.entries.lock().unwrap_or_else(|v| v.into_inner());
        if entries.len() >= 32 || entries.contains_key(&id) || self.shutdown.is_cancelled() {
            return None;
        }
        let entry = Arc::new(Entry::new(connection_id));
        entries.insert(id, entry.clone());
        Some(entry)
    }
    pub fn get(&self, id: ObjectId) -> Option<Arc<Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .get(&id)
            .cloned()
    }
    pub fn for_connection(&self, id: ObjectId) -> Vec<Arc<Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .values()
            .filter(|v| v.connection_id == id)
            .cloned()
            .collect()
    }
    pub fn remove(&self, id: ObjectId) {
        self.entries
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .remove(&id);
    }
    pub fn stop_all(&self) {
        self.shutdown.cancel();
        for entry in self
            .entries
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .values()
        {
            entry.stop.cancel();
        }
    }
    pub async fn wait_shutdown(&self) {
        self.stop_all();
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|v| v.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        futures_util::future::join_all(entries.into_iter().map(|entry| async move {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), entry.wait()).await;
        }))
        .await;
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.stop_all();
    }
}
