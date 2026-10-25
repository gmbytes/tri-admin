//! Isolated server for UI acceptance tests. Never points at a normal application database.
use admin_api::{
    bootstrap::{AdminInput, bootstrap_database},
    build_router,
    config::Config,
    state::AppState,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let name = std::env::var("TRI_E2E_DATABASE")?;
    if !name.starts_with("tri_admin_e2e_")
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("E2E database name must use tri_admin_e2e_ prefix".into());
    }
    let password = std::fs::read_to_string(std::env::var("TRI_E2E_PASSWORD_FILE")?)?;
    let config = Config::from_lookup(|key| match key {
        "TRI_MONGODB_DATABASE" => Some(name.clone()),
        "TRI_ADMIN_BIND" => Some("127.0.0.1:0".into()),
        _ => std::env::var(key).ok(),
    })?;
    let state = AppState::from_config(config).await?;
    bootstrap_database(
        state.require_database()?,
        &AdminInput {
            username: "admin".into(),
            password,
        },
    )
    .await?;
    let listener = tokio::net::TcpListener::bind(state.config.bind).await?;
    let address = listener.local_addr()?;
    std::fs::write(
        std::env::var("TRI_E2E_META_FILE")?,
        serde_json::to_vec(&serde_json::json!({"api":format!("http://{address}/api/v1")}))?,
    )?;
    println!("Isolated E2E service ready on {}", listener.local_addr()?);
    let database = state.require_database()?.clone();
    axum::serve(listener,build_router(state).into_make_service_with_connect_info::<std::net::SocketAddr>()).with_graceful_shutdown(async{
        tokio::select!{_=tokio::signal::ctrl_c()=>{},_=tokio::task::spawn_blocking(||{let mut command=String::new();let _=std::io::stdin().read_line(&mut command);})=>{}}
    }).await?;
    database.drop().await?;
    Ok(())
}
