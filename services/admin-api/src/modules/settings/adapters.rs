use super::{
    dto::{Credentials, Integration, MailSecurity},
    network,
};
use crate::config::Config;
use std::{
    io::{Read, Write},
    time::Duration,
};
pub struct Outcome {
    pub success: bool,
    pub scope: &'static str,
    pub error: Option<&'static str>,
    pub status: Option<u16>,
}
impl Outcome {
    pub fn ok(scope: &'static str) -> Self {
        Self {
            success: true,
            scope,
            error: None,
            status: None,
        }
    }
    pub fn failed(scope: &'static str, error: &'static str) -> Self {
        Self {
            success: false,
            scope,
            error: Some(error),
            status: None,
        }
    }
}
fn response(scope: &'static str, status: u16) -> Outcome {
    Outcome {
        success: (200..300).contains(&status),
        scope,
        error: if (200..300).contains(&status) {
            None
        } else if (300..400).contains(&status) {
            Some("REDIRECT_REJECTED")
        } else if [401, 403].contains(&status) {
            Some("UPSTREAM_AUTH_REJECTED")
        } else {
            Some("UPSTREAM_STATUS_REJECTED")
        },
        status: Some(status),
    }
}
async fn http(
    config: &Config,
    base: &str,
    secret: Option<&Credentials>,
) -> Result<Outcome, &'static str> {
    let url = url::Url::parse(base).map_err(|_| "DESTINATION_BLOCKED")?;
    let client = network::http_client(&url, config).await?;
    let mut request = client.head(url);
    if let Some(Credentials::Http { header_name, value }) = secret {
        let name = reqwest::header::HeaderName::from_bytes(header_name.as_bytes())
            .map_err(|_| "CREDENTIALS_INVALID")?;
        let mut value =
            reqwest::header::HeaderValue::from_str(value).map_err(|_| "CREDENTIALS_INVALID")?;
        value.set_sensitive(true);
        request = request.header(name, value);
    }
    let result = request.send().await.map_err(|error| {
        if error.is_timeout() {
            "CHECK_TIMEOUT"
        } else {
            "HTTP_CONNECTION_FAILED"
        }
    })?;
    Ok(response("httpHead", result.status().as_u16()))
}
async fn s3(
    config: &Config,
    public: &Integration,
    secret: Option<&Credentials>,
) -> Result<Outcome, &'static str> {
    let Integration::S3 {
        endpoint,
        bucket,
        region,
        path_style,
        ..
    } = public
    else {
        return Err("CONFIGURATION_INVALID");
    };
    let Some(Credentials::S3 {
        access_key_id,
        secret_access_key,
        session_token,
    }) = secret
    else {
        return Err("CREDENTIALS_INVALID");
    };
    let mut url = url::Url::parse(endpoint).map_err(|_| "DESTINATION_BLOCKED")?;
    if *path_style {
        url.path_segments_mut()
            .map_err(|_| "DESTINATION_BLOCKED")?
            .pop_if_empty()
            .push(bucket);
    } else {
        let host = network::url_host(&url)?;
        if host.parse::<std::net::IpAddr>().is_ok() {
            return Err("S3_VIRTUAL_HOST_INVALID");
        }
        url.set_host(Some(&format!("{bucket}.{host}")))
            .map_err(|_| "DESTINATION_BLOCKED")?;
    }
    let client = network::http_client(&url, config).await?;
    use aws_sigv4::{
        http_request::{
            PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest,
            SigningSettings, UriPathNormalizationMode, sign,
        },
        sign::v4,
    };
    let identity = aws_credential_types::Credentials::new(
        access_key_id,
        secret_access_key,
        session_token.clone(),
        None,
        "tri-admin-profile",
    )
    .into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
    settings.percent_encoding_mode = PercentEncodingMode::Single;
    settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name("s3")
        .time(std::time::SystemTime::now())
        .settings(settings)
        .build()
        .map_err(|_| "S3_SIGNING_FAILED")?
        .into();
    let signable = SignableRequest::new(
        "HEAD",
        url.as_str(),
        std::iter::empty(),
        SignableBody::Bytes(&[]),
    )
    .map_err(|_| "S3_SIGNING_FAILED")?;
    let (instructions, _) = sign(signable, &params)
        .map_err(|_| "S3_SIGNING_FAILED")?
        .into_parts();
    let mut signed = axum::http::Request::builder()
        .method("HEAD")
        .uri(url.as_str())
        .body(())
        .map_err(|_| "S3_SIGNING_FAILED")?;
    instructions.apply_to_request_http1x(&mut signed);
    let mut headers = signed.headers().clone();
    for name in ["authorization", "x-amz-security-token"] {
        if let Some(value) = headers.get_mut(name) {
            value.set_sensitive(true);
        }
    }
    let result = client
        .head(url)
        .headers(headers)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "CHECK_TIMEOUT"
            } else {
                "S3_CONNECTION_FAILED"
            }
        })?;
    Ok(response("s3BucketHead", result.status().as_u16()))
}
async fn mongo(
    public: &Integration,
    secret: Option<&Credentials>,
    config: &Config,
) -> Result<Outcome, &'static str> {
    let Integration::MongoDb {
        hosts,
        database,
        auth_database,
        replica_set,
        tls,
        direct_connection,
    } = public
    else {
        return Err("CONFIGURATION_INVALID");
    };
    for item in hosts {
        network::resolve(&item.host, item.port).await?;
    }
    use mongodb::options::{ClientOptions, Credential, ServerAddress, Tls, TlsOptions};
    let addresses = hosts
        .iter()
        .map(|item| ServerAddress::Tcp {
            host: item.host.clone(),
            port: Some(item.port),
        })
        .collect::<Vec<_>>();
    let mut options = ClientOptions::builder().hosts(addresses).build();
    options.app_name = Some("tri-admin-configuration-check".into());
    options.repl_set_name = replica_set.clone();
    options.direct_connection = Some(*direct_connection);
    options.server_selection_timeout = Some(Duration::from_secs(3));
    options.connect_timeout = Some(Duration::from_secs(3));
    if *tls {
        let mut tls_options = TlsOptions::builder().build();
        tls_options.ca_file_path = config.outbound_ca_file.clone();
        options.tls = Some(Tls::Enabled(tls_options));
    } else {
        options.tls = Some(Tls::Disabled);
    }
    if let Some(Credentials::MongoDb { username, password }) = secret {
        options.credential = Some(
            Credential::builder()
                .username(username.clone())
                .password(password.clone())
                .source(auth_database.clone())
                .build(),
        );
    }
    let client =
        mongodb::Client::with_options(options).map_err(|_| "MONGODB_CONFIGURATION_INVALID")?;
    match client
        .database(database)
        .run_command(mongodb::bson::doc! {"ping":1,"maxTimeMS":2000})
        .await
    {
        Ok(_) => Ok(Outcome::ok("mongodbPing")),
        Err(error) => Err(match &*error.kind {
            mongodb::error::ErrorKind::Authentication { .. } => "MONGODB_AUTH_REJECTED",
            mongodb::error::ErrorKind::Command(command) if [13, 18].contains(&command.code) => {
                "MONGODB_AUTH_REJECTED"
            }
            _ => "MONGODB_CONNECTION_FAILED",
        }),
    }
}
async fn smtp(
    config: &Config,
    public: &Integration,
    secret: Option<&Credentials>,
) -> Result<Outcome, &'static str> {
    let Integration::Smtp {
        host,
        port,
        security,
        ..
    } = public
    else {
        return Err("CONFIGURATION_INVALID");
    };
    let addresses = network::resolve(host, *port).await?;
    use lettre::{
        AsyncSmtpTransport, Tokio1Executor,
        transport::smtp::{
            authentication::Credentials as MailCredentials,
            client::{Certificate, Tls, TlsParameters},
        },
    };
    let mut parameters = TlsParameters::builder(host.clone());
    for der in network::certificates(config).await? {
        parameters = parameters.add_root_certificate(
            Certificate::from_der(der).map_err(|_| "TLS_CONFIGURATION_INVALID")?,
        );
    }
    let parameters = parameters
        .build()
        .map_err(|_| "TLS_CONFIGURATION_INVALID")?;
    for address in addresses {
        let mut builder =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(address.ip().to_string())
                .port(*port)
                .timeout(Some(Duration::from_secs(2)))
                .tls(match security {
                    MailSecurity::Plain => Tls::None,
                    MailSecurity::Tls => Tls::Wrapper(parameters.clone()),
                    MailSecurity::StartTls => Tls::Required(parameters.clone()),
                });
        if let Some(Credentials::Smtp { username, password }) = secret {
            builder = builder.credentials(MailCredentials::new(username.clone(), password.clone()));
        }
        match builder.build::<Tokio1Executor>().test_connection().await {
            Ok(true) => return Ok(Outcome::ok("smtpSession")),
            Ok(false) => return Err("SMTP_SESSION_REJECTED"),
            Err(error)
                if error.status().is_some_and(|code| {
                    ["530", "534", "535", "454"].contains(&code.to_string().as_str())
                }) =>
            {
                return Err("SMTP_AUTH_REJECTED");
            }
            Err(error) if error.status().is_some() => return Err("SMTP_SESSION_REJECTED"),
            Err(_) => {}
        }
    }
    Err("SMTP_CONNECTION_FAILED")
}
fn local(root: &str, subdirectory: &str) -> Result<Outcome, &'static str> {
    use cap_std::{
        ambient_authority,
        fs::{Dir, OpenOptions},
    };
    Dir::create_ambient_dir_all(root, ambient_authority()).map_err(|_| "LOCAL_ROOT_UNAVAILABLE")?;
    let root =
        Dir::open_ambient_dir(root, ambient_authority()).map_err(|_| "LOCAL_ROOT_UNAVAILABLE")?;
    root.create_dir_all(subdirectory)
        .map_err(|_| "LOCAL_PATH_REJECTED")?;
    let directory = root
        .open_dir(subdirectory)
        .map_err(|_| "LOCAL_PATH_REJECTED")?;
    let name = format!(".tri-admin-probe-{}", uuid::Uuid::new_v4().simple());
    let data = uuid::Uuid::new_v4().to_string();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = directory
        .open_with(&name, &options)
        .map_err(|_| "LOCAL_WRITE_FAILED")?;
    let written = file
        .write_all(data.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| "LOCAL_WRITE_FAILED");
    drop(file);
    let checked = written.and_then(|_| {
        let mut value = String::new();
        directory
            .open(&name)
            .and_then(|file| file.take(128).read_to_string(&mut value))
            .map_err(|_| "LOCAL_READ_FAILED")?;
        if value != data {
            return Err("LOCAL_READ_FAILED");
        }
        Ok(())
    });
    directory
        .remove_file(&name)
        .map_err(|_| "LOCAL_CLEANUP_FAILED")?;
    checked?;
    Ok(Outcome::ok("localWriteReadDelete"))
}
pub async fn run(config: &Config, public: &Integration, secret: Option<&Credentials>) -> Outcome {
    let scope = match public {
        Integration::Http { .. } => "httpHead",
        Integration::S3 { .. } => "s3BucketHead",
        Integration::MongoDb { .. } => "mongodbPing",
        Integration::Smtp { .. } => "smtpSession",
        Integration::LocalStorage { .. } => "localWriteReadDelete",
    };
    let result = match public {
        Integration::Http { base_url } => http(config, base_url, secret).await,
        Integration::S3 { .. } => s3(config, public, secret).await,
        Integration::MongoDb { .. } => mongo(public, secret, config).await,
        Integration::Smtp { .. } => smtp(config, public, secret).await,
        Integration::LocalStorage { subdirectory } => {
            let root = config.storage_root.clone();
            let directory = subdirectory.clone();
            tokio::task::spawn_blocking(move || local(&root, &directory))
                .await
                .unwrap_or(Err("LOCAL_CHECK_FAILED"))
        }
    };
    result.unwrap_or_else(|error| Outcome::failed(scope, error))
}
