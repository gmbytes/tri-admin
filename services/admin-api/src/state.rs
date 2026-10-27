use crate::{common::error::AppError, config::Config};
use mongodb::{Client, Database, options::ClientOptions};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub database: Option<Database>,
    pub(crate) mcp: Arc<crate::modules::mcp::registry::Registry>,
    pub(crate) connection_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) plugin_compile_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) ai_worker_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) ai_worker_shutdown: tokio_util::sync::CancellationToken,
    pub(crate) data_tool_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) data_tool_shutdown: tokio_util::sync::CancellationToken,
    pub(crate) export_worker_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) export_maintenance_slot: Arc<tokio::sync::Semaphore>,
    pub(crate) export_worker_shutdown: tokio_util::sync::CancellationToken,
    pub(crate) open_access_shutdown: tokio_util::sync::CancellationToken,
    pub(crate) file_transfer_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) file_worker_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) file_maintenance_slot: Arc<tokio::sync::Semaphore>,
    pub(crate) file_worker_shutdown: tokio_util::sync::CancellationToken,
    pub(crate) file_next_staging_sweep: Arc<std::sync::atomic::AtomicI64>,
}

impl AppState {
    pub fn new(config: Config, database: Option<Database>) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self {
            config: Arc::new(config),
            database,
            mcp: Arc::new(crate::modules::mcp::registry::Registry::new()),
            connection_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            plugin_compile_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            ai_worker_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            ai_worker_shutdown: tokio_util::sync::CancellationToken::new(),
            data_tool_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            data_tool_shutdown: tokio_util::sync::CancellationToken::new(),
            export_worker_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            export_maintenance_slot: Arc::new(tokio::sync::Semaphore::new(1)),
            export_worker_shutdown: tokio_util::sync::CancellationToken::new(),
            open_access_shutdown: tokio_util::sync::CancellationToken::new(),
            file_transfer_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            file_worker_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            file_maintenance_slot: Arc::new(tokio::sync::Semaphore::new(1)),
            file_worker_shutdown: tokio_util::sync::CancellationToken::new(),
            file_next_staging_sweep: Arc::new(std::sync::atomic::AtomicI64::new(0)),
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
