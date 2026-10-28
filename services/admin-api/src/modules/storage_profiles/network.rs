use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
pub fn allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_unspecified() && !ip.is_multicast() && !ip.is_link_local() && !ip.is_broadcast()
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                allowed(IpAddr::V4(ip))
            } else {
                !ip.is_unspecified() && !ip.is_multicast() && !ip.is_unicast_link_local()
            }
        }
    }
}
pub async fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, &'static str> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if allowed(ip) {
            Ok(vec![SocketAddr::new(ip, port)])
        } else {
            Err("DESTINATION_BLOCKED")
        };
    }
    let addresses = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| "DNS_TIMEOUT")?
    .map_err(|_| "DNS_FAILED")?
    .take(17)
    .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.len() > 16 {
        return Err("DNS_FAILED");
    }
    if addresses.iter().any(|address| !allowed(address.ip())) {
        return Err("DESTINATION_BLOCKED");
    }
    Ok(addresses)
}
pub fn url_host(url: &url::Url) -> Result<String, &'static str> {
    match url.host() {
        Some(url::Host::Domain(value)) => Ok(value.to_owned()),
        Some(url::Host::Ipv4(value)) => Ok(value.to_string()),
        Some(url::Host::Ipv6(value)) => Ok(value.to_string()),
        None => Err("DESTINATION_BLOCKED"),
    }
}
pub async fn certificates(config: &crate::config::Config) -> Result<Vec<Vec<u8>>, &'static str> {
    let Some(path) = &config.outbound_ca_file else {
        return Ok(vec![]);
    };
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| "TLS_CONFIGURATION_INVALID")?;
    if bytes.len() > 65536 {
        return Err("TLS_CONFIGURATION_INVALID");
    }
    let certs = rustls_pemfile::certs(&mut std::io::Cursor::new(bytes))
        .map(|value| {
            value
                .map(|value| value.as_ref().to_vec())
                .map_err(|_| "TLS_CONFIGURATION_INVALID")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err("TLS_CONFIGURATION_INVALID");
    }
    Ok(certs)
}
pub async fn http_client(
    url: &url::Url,
    config: &crate::config::Config,
) -> Result<reqwest::Client, &'static str> {
    http_client_with_timeout(url, config, Duration::from_secs(5)).await
}
pub(crate) async fn http_client_with_timeout(
    url: &url::Url,
    config: &crate::config::Config,
    timeout: Duration,
) -> Result<reqwest::Client, &'static str> {
    let host = url_host(url)?;
    let port = url.port_or_known_default().ok_or("DESTINATION_BLOCKED")?;
    let addresses = resolve(&host, port).await?;
    if url.scheme() != "https"
        && (url.scheme() != "http" || addresses.iter().any(|address| !address.ip().is_loopback()))
    {
        return Err("DESTINATION_BLOCKED");
    }
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(timeout)
        .read_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(3))
        .resolve_to_addrs(&host, &addresses)
        .pool_max_idle_per_host(0);
    for bytes in certificates(config).await? {
        let certificate =
            reqwest::Certificate::from_der(&bytes).map_err(|_| "TLS_CONFIGURATION_INVALID")?;
        builder = builder.tls_certs_merge([certificate]);
    }
    builder.build().map_err(|_| "TLS_CONFIGURATION_INVALID")
}
