use crate::{common::error::AppError, config::Config};
use mongodb::{Client, Database, options::ClientOptions};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub database: Option<Database>,
    pub(crate) connection_slots: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(config: Config, database: Option<Database>) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self {
            config: Arc::new(config),
            database,
            connection_slots: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }

    pub async fn from_config(config: Config) -> Result<Self, AppError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let _ = crate::common::password::dummy_hash()?;
        let database = if let Some(uri) = config.mongodb_uri() {
            let mut options = ClientOptions::parse(uri)
                .await
                .map_err(|_| AppError::Configuration("MongoDB 连接配置无效".into()))?;
            options.app_name = Some("tri-admin".into());
            options.server_selection_timeout = Some(Duration::from_secs(3));
            let client = Client::with_options(options)
                .map_err(|_| AppError::Configuration("MongoDB 连接配置无效".into()))?;
            Some(client.database(&config.database_name))
        } else {
            None
        };
        Ok(Self::new(config, database))
    }

    pub fn require_database(&self) -> Result<&Database, AppError> {
        self.database.as_ref().ok_or(AppError::NotReady)
    }
}
