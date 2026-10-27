//! Trusted-host HTTP transport. The host supplies a DNS-pinned, no-proxy,
//! no-redirect, no-retry client and resolves credential references before this call.
use crate::session::{Client, Error, Limits, SessionControl};
use futures_util::{StreamExt, stream::BoxStream};
use reqwest::{
    Client as HttpClient,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use rmcp::{
    model::{
        ClientCapabilities, ClientConfig, ClientJsonRpcMessage, Implementation, ProtocolVersion,
        ServerJsonRpcMessage,
    },
    service::{ClientLifecycleMode, ClientServiceExt},
    transport::{
        StreamableHttpClientTransport,
        common::client_side_sse::NeverRetry,
        streamable_http_client::{
            SseError, StreamableHttpClient, StreamableHttpClientTransportConfig,
            StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use sse_stream::{Sse, SseStream};
use std::{collections::HashMap, sync::Arc, time::Duration};

// Intentionally no Debug/Serialize: headers contain the host-resolved credentials.
pub struct HttpSpec {
    pub endpoint: String,
    pub client: HttpClient,
    pub headers: HeaderMap,
}
fn reserved(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "accept"
            | "content-type"
            | "content-length"
            | "host"
            | "connection"
            | "transfer-encoding"
            | "last-event-id"
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authorization"
            | "forwarded"
    ) || name.as_str().starts_with("mcp-")
        || name.as_str().starts_with("x-forwarded-")
}
pub async fn connect(mut spec: HttpSpec, limits: Limits) -> Result<Client, Error> {
    limits.validate()?;
    let url = reqwest::Url::parse(&spec.endpoint).map_err(|_| Error::InvalidInput)?;
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if spec.endpoint.len() > 2048
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || spec.headers.len() > 16
        || spec.headers.keys().any(reserved)
        || spec
            .headers
            .values()
            .map(|v| v.as_bytes().len())
            .sum::<usize>()
            > 16384
    {
        return Err(Error::InvalidInput);
    }
    for value in spec.headers.values_mut() {
        value.set_sensitive(true)
    }
    let endpoint: Arc<str> = url.as_str().into();
    let control = SessionControl::new();
    let adapter = BoundedHttp {
        client: spec.client,
        headers: spec.headers,
        endpoint: endpoint.clone(),
        max: limits.max_frame_bytes,
        control: control.clone(),
    };
    let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint)
        .max_concurrent_requests(limits.max_concurrent_calls)
        .control_request_timeout(Duration::from_millis(200))
        .max_sse_event_size(limits.max_frame_bytes)
        .reinit_on_expired_session(false);
    config.retry_config = Arc::new(NeverRetry::default());
    config.channel_buffer_capacity = limits.max_concurrent_calls + 1;
    let transport = StreamableHttpClientTransport::with_client(adapter, config);
    let config = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("tri-admin", env!("CARGO_PKG_VERSION")),
    );
    match tokio::time::timeout(
        limits.startup_timeout,
        config.serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        ),
    )
    .await
    {
        Ok(Ok(service)) => Client::from_service(service, limits, control).await,
        failure => {
            control.stop.cancel();
            Err(if failure.is_err() {
                Error::StartTimeout
            } else {
                Error::StartRejected
            })
        }
    }
}
#[derive(Clone)]
struct BoundedHttp {
    client: HttpClient,
    headers: HeaderMap,
    endpoint: Arc<str>,
    max: usize,
    control: Arc<SessionControl>,
}
#[derive(Debug, thiserror::Error)]
#[error("MCP HTTP 传输拒绝或中断")]
struct HttpError;
type Failure = StreamableHttpError<HttpError>;
fn failed() -> Failure {
    StreamableHttpError::Client(HttpError)
}
impl BoundedHttp {
    async fn post(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        auth: Option<String>,
        custom: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        if uri != self.endpoint
            || session.is_some()
            || auth.is_some()
            || !matches!(message, ClientJsonRpcMessage::Request(_))
        {
            return Err(failed());
        }
        let bytes = serde_json::to_vec(&message).map_err(|_| failed())?;
        if bytes.len() > self.max {
            return Err(failed());
        }
        let mut headers = self.headers.clone();
        for (name, value) in custom {
            if headers.contains_key(&name) {
                return Err(failed());
            }
            headers.insert(name, value);
        }
        let sending = self
            .client
            .post(uri.as_ref())
            .headers(headers)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(bytes)
            .send();
        let response = tokio::select! {result=sending=>result.map_err(|_|failed())?,_=self.control.stop.cancelled()=>return Err(failed())};
        let status = response.status();
        if status.is_redirection()
            || response
                .content_length()
                .is_some_and(|v| v > self.max as u64)
            || response
                .headers()
                .get("mcp-protocol-version")
                .is_some_and(|v| v != "2026-07-28")
        {
            return Err(failed());
        }
        let kind = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if kind == "text/event-stream" && status.is_success() {
            let max = self.max;
            let stop = self.control.stop.clone();
            let bytes = response
                .bytes_stream()
                .scan((0usize, false), move |state, item| {
                    if state.1 {
                        return std::future::ready(None);
                    }
                    let item = match item {
                        Ok(bytes) if bytes.len() <= max.saturating_sub(state.0) => {
                            state.0 += bytes.len();
                            Ok(bytes)
                        }
                        _ => {
                            state.1 = true;
                            Err(HttpError)
                        }
                    };
                    std::future::ready(Some(item))
                })
                .take_until(stop.cancelled_owned());
            return Ok(StreamableHttpPostResponse::Sse(
                SseStream::from_bytes_stream(bytes).boxed(),
                None,
            ));
        }
        if kind != "application/json" {
            return Err(failed());
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        loop {
            let chunk = tokio::select! {chunk=stream.next()=>chunk,_=self.control.stop.cancelled()=>return Err(failed())};
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|_| failed())?;
            if chunk.len() > self.max.saturating_sub(body.len()) {
                return Err(failed());
            }
            body.extend_from_slice(&chunk);
        }
        let response: ServerJsonRpcMessage = serde_json::from_slice(&body).map_err(|_| failed())?;
        if matches!(response, ServerJsonRpcMessage::Request(_))
            || (!status.is_success() && !matches!(response, ServerJsonRpcMessage::Error(_)))
        {
            return Err(failed());
        }
        Ok(StreamableHttpPostResponse::Json(response, None))
    }
}
impl StreamableHttpClient for BoundedHttp {
    type Error = HttpError;
    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        auth: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        self.post(uri, message, session, auth, headers).await
    }
    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        auth: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
        _max: usize,
    ) -> Result<StreamableHttpPostResponse, Failure> {
        self.post(uri, message, session, auth, headers).await
    }
    async fn delete_session(
        &self,
        _uri: Arc<str>,
        _session: Arc<str>,
        _auth: Option<String>,
        _headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Failure> {
        Err(failed())
    }
    async fn get_stream(
        &self,
        _uri: Arc<str>,
        _session: Option<Arc<str>>,
        _last: Option<String>,
        _auth: Option<String>,
        _headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Failure> {
        Err(failed())
    }
}
