use admin_api::{
    bootstrap::{AdminInput, bootstrap_database},
    build_router,
    config::Config,
    state::AppState,
    storage::schema::migrate_database,
};
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "admin_api=info,tower_http=info".into()),
        )
        .try_init();
    std::panic::set_hook(Box::new(|info| {
        let request_id = admin_api::common::observability::request_id().unwrap_or_default();
        if let Some(location) = info.location() {
            tracing::error!(
                request_id,
                file = location.file(),
                line = location.line(),
                "handler panicked"
            );
        } else {
            tracing::error!(request_id, "handler panicked");
        }
    }));
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load()?;
    let state = AppState::from_config(config).await?;
    match std::env::args().nth(1).as_deref().unwrap_or("serve") {
        "migrate" => {
            migrate_database(state.require_database()?).await?;
            println!("数据库模型和索引已就绪");
        }
        "bootstrap" => {
            let password = rpassword::prompt_password("管理员密码（至少 12 个字符）：")?;
            let username =
                std::env::var("TRI_BOOTSTRAP_USERNAME").unwrap_or_else(|_| "admin".into());
            let report = bootstrap_database(
                state.require_database()?,
                &AdminInput { username, password },
            )
            .await?;
            println!("{}", serde_json::to_string(&report)?);
        }
        "maintain-files" => {
            if !admin_api::storage::schema::is_initialized(state.require_database()?).await? {
                return Err("文件维护需要先完成数据库迁移和初始化".into());
            }
            let report = admin_api::modules::files::jobs::maintenance_once(&state).await?;
            println!("{}", serde_json::to_string(&report)?);
        }
        "maintain-data-tools" => {
            let report = admin_api::modules::data_tools::retention::maintenance_once(
                state.require_database()?,
            )
            .await?;
            println!("{}", serde_json::to_string(&report)?);
        }
        "maintain-exports" => {
            let report =
                admin_api::modules::data_tools::export_jobs::maintenance::run_once(&state).await?;
            println!("{}", serde_json::to_string(&report)?);
        }
        "serve" => {
            let listener = tokio::net::TcpListener::bind(state.config.bind).await?;
            tracing::info!(address=%listener.local_addr()?,"admin service listening");
            let workers = admin_api::modules::files::jobs::start_service(state.clone());
            let data_workers =
                admin_api::modules::data_tools::retention::start_service(state.clone());
            let export_workers =
                admin_api::modules::data_tools::export_jobs::worker::start_service(state.clone());
            let shutdown_state = state.clone();
            let worker_state = state.clone();
            let result = axum::serve(
                listener,
                build_router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                shutdown().await;
                admin_api::modules::files::jobs::stop_service(&shutdown_state);
                admin_api::modules::data_tools::retention::stop_service(&shutdown_state);
                admin_api::modules::data_tools::export_jobs::worker::stop_service(&shutdown_state);
            })
            .await;
            admin_api::modules::files::jobs::stop_service(&worker_state);
            admin_api::modules::data_tools::retention::stop_service(&worker_state);
            admin_api::modules::data_tools::export_jobs::worker::stop_service(&worker_state);
            let _ = workers.await;
            let _ = data_workers.await;
            let _ = export_workers.await;
            result?;
        }
        _ => {
            return Err(
                "Usage: admin-api [serve|migrate|bootstrap|maintain-files|maintain-data-tools|maintain-exports]"
                    .into(),
            );
        }
    }
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    if let Ok(mut signal) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = signal.recv()=>{}};
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}
