use crate::{Config, Error, State, router, schema::Schema};
pub async fn run(schema: Schema) -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();
    let name = std::env::var("TRI_MONGODB_DATABASE")?;
    if !(3..=64).contains(&name.len())
        || !name.bytes().all(|v| v.is_ascii_alphanumeric() || v == b'_')
    {
        return Err(Error::Validation.into());
    }
    let database = mongodb::Client::with_uri_str(std::env::var("TRI_MONGODB_URI")?)
        .await
        .map_err(|_| Error::NotReady)?
        .database(&name);
    let config = Config::new(
        std::env::var("TRI_SIGNING_KEY")?,
        std::env::var("TRI_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".into()),
    )?;
    let state = State::new(database, schema, config)?;
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str).unwrap_or("serve") {
        "migrate" if args.len() == 1 => {
            state.migrate().await?;
            println!("资源模型和索引已就绪");
        }
        "create-admin" => {
            if args
                .iter()
                .skip(1)
                .any(|v| v != "--read-only" && v != "--self")
            {
                return Err(Error::Validation.into());
            }
            let username = std::env::var("TRI_ADMIN_USERNAME").unwrap_or_else(|_| "admin".into());
            let password = rpassword::prompt_password("管理员密码（至少 12 个字符）：")?;
            let view = state
                .provision_admin(
                    &username,
                    &password,
                    !args.iter().any(|v| v == "--read-only"),
                    !args.iter().any(|v| v == "--self"),
                )
                .await?;
            println!("已创建管理员 {}", view.id);
        }
        "serve" if args.len() <= 1 => {
            state.ready().await?;
            let bind: std::net::SocketAddr = std::env::var("TRI_BIND")
                .unwrap_or_else(|_| "127.0.0.1:18089".into())
                .parse()
                .map_err(|_| Error::Validation)?;
            let listener = tokio::net::TcpListener::bind(bind).await?;
            println!(
                "{}",
                serde_json::json!({"listening":listener.local_addr()?.to_string()})
            );
            axum::serve(
                listener,
                router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        }
        _ => return Err(Error::Validation.into()),
    }
    Ok(())
}
