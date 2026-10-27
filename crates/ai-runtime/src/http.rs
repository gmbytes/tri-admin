//! Trusted-host transport: the host supplies a DNS-pinned client with redirects,
//! retries and proxies disabled, plus credentials resolved under current policy.
//! Every POST is attempted once. Any failure after submission remains ambiguous.
pub use crate::request::{CreateRequest, Failure, FailureKind, JsonFormat, Policy, Reference};
use crate::{Error, Limits, ResponseState, Status, Update, frames};
use bytes::Bytes;
use futures_util::{StreamExt, stream::BoxStream};
use reqwest::{
    Client, Method, Url,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName},
};
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub struct HttpSpec {
    pub endpoint: String,
    pub client: Client,
    pub headers: HeaderMap,
}
pub struct Adapter {
    endpoint: Url,
    client: Client,
    headers: HeaderMap,
    limits: Limits,
    policy: Policy,
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
            | "te"
            | "trailer"
            | "upgrade"
            | "proxy-authorization"
            | "forwarded"
            | "last-event-id"
    ) || name.as_str().starts_with("x-forwarded-")
}
/// Pure validation for approved host bindings; it performs no network request.
pub fn validate_definition(raw: &str, headers: &HeaderMap) -> Result<Url, Failure> {
    let endpoint = Url::parse(raw).map_err(|_| Failure::input())?;
    let loopback = endpoint.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if raw.len() > 2048
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.host_str().is_none()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || !endpoint.path().ends_with("/responses")
        || !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
        || headers.len() > 16
        || headers.keys().any(reserved)
        || headers
            .values()
            .map(|value| value.as_bytes().len())
            .sum::<usize>()
            > 16384
    {
        return Err(Failure::input());
    }
    Ok(endpoint)
}
impl Adapter {
    pub fn new(mut spec: HttpSpec, limits: Limits, policy: Policy) -> Result<Self, Failure> {
        let limits = limits.validate().map_err(|_| Failure::input())?;
        let policy = policy.validate()?;
        let endpoint = validate_definition(&spec.endpoint, &spec.headers)?;
        for value in spec.headers.values_mut() {
            value.set_sensitive(true);
        }
        Ok(Self {
            endpoint,
            client: spec.client,
            headers: spec.headers,
            limits,
            policy,
        })
    }
    pub async fn create(
        &self,
        request: CreateRequest,
        cancel: CancellationToken,
    ) -> Result<Exchange, Failure> {
        let body = request.body()?;
        let state = ResponseState::new(self.limits).map_err(|_| Failure::input())?;
        self.send(
            Operation {
                method: Method::POST,
                url: self.endpoint.clone(),
                body: Some(body),
                stream: request.stream,
                expected_id: None,
            },
            state,
            cancel,
        )
        .await
    }
    fn target(&self, reference: &Reference, state: &ResponseState) -> Result<Url, Failure> {
        if !crate::responses::identifier(&reference.id)
            || state.response_id().is_some_and(|id| id != reference.id)
        {
            return Err(Failure::input());
        }
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .map_err(|_| Failure::input())?
            .push(&reference.id);
        Ok(url)
    }
    pub async fn retrieve(
        &self,
        reference: &Reference,
        state: ResponseState,
        cancel: CancellationToken,
    ) -> Result<Exchange, Failure> {
        let url = self.target(reference, &state)?;
        self.send(
            Operation {
                method: Method::GET,
                url,
                body: None,
                stream: false,
                expected_id: Some(reference.id.clone()),
            },
            state,
            cancel,
        )
        .await
    }
    pub async fn resume(
        &self,
        reference: &Reference,
        state: ResponseState,
        cancel: CancellationToken,
    ) -> Result<Exchange, Failure> {
        let mut url = self.target(reference, &state)?;
        if !reference.background
            || !reference.streamed
            || state.response_id() != Some(reference.id.as_str())
            || state.status().is_some()
        {
            return Err(Failure::input());
        }
        url.query_pairs_mut().append_pair("stream", "true");
        if let Some(cursor) = state.sequence() {
            url.query_pairs_mut()
                .append_pair("starting_after", &cursor.to_string());
        }
        self.send(
            Operation {
                method: Method::GET,
                url,
                body: None,
                stream: true,
                expected_id: Some(reference.id.clone()),
            },
            state,
            cancel,
        )
        .await
    }
    pub async fn cancel(
        &self,
        reference: &Reference,
        state: ResponseState,
        cancel: CancellationToken,
    ) -> Result<Exchange, Failure> {
        let mut url = self.target(reference, &state)?;
        if !reference.background || state.status().is_some() {
            return Err(Failure::input());
        }
        url.path_segments_mut()
            .map_err(|_| Failure::input())?
            .push("cancel");
        self.send(
            Operation {
                method: Method::POST,
                url,
                body: None,
                stream: false,
                expected_id: Some(reference.id.clone()),
            },
            state,
            cancel,
        )
        .await
    }
    async fn send(
        &self,
        operation: Operation,
        state: ResponseState,
        cancel: CancellationToken,
    ) -> Result<Exchange, Failure> {
        let mut state = ResponseState::restore(
            &state.checkpoint().map_err(|error| Failure {
                kind: FailureKind::Protocol(error),
                may_have_been_sent: false,
            })?,
            self.limits,
        )
        .map_err(|error| Failure {
            kind: FailureKind::Protocol(error),
            may_have_been_sent: false,
        })?;
        state.begin_read();
        if cancel.is_cancelled() {
            return Err(Failure {
                kind: FailureKind::Cancelled,
                may_have_been_sent: false,
            });
        }
        let deadline = Instant::now() + self.policy.total_timeout;
        let headers_deadline = std::cmp::min(deadline, Instant::now() + self.policy.idle_timeout);
        let mut request = self
            .client
            .request(operation.method, operation.url)
            .headers(self.headers.clone())
            .header(
                ACCEPT,
                if operation.stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            );
        if let Some(body) = operation.body {
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }
        let response = tokio::select! {biased;
            _=cancel.cancelled()=>return Err(Failure::sent(FailureKind::Cancelled)),
            result=tokio::time::timeout_at(headers_deadline,request.send())=>match result{Ok(Ok(response))=>response,Ok(Err(_))=>return Err(Failure::sent(FailureKind::Transport)),Err(_)=>return Err(Failure::sent(FailureKind::Timeout))}
        };
        if !response.status().is_success() {
            return Err(Failure::sent(FailureKind::Status(
                response.status().as_u16(),
            )));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or("")
            .trim();
        if !content_type.eq_ignore_ascii_case(if operation.stream {
            "text/event-stream"
        } else {
            "application/json"
        }) {
            return Err(Failure::reading(Error::Protocol));
        }
        if response
            .content_length()
            .is_some_and(|size| size > self.limits.max_stream_bytes as u64)
        {
            return Err(Failure::reading(Error::Limit));
        }
        let source = bounded_source(response, deadline, self.policy.idle_timeout, cancel.clone());
        let body = if operation.stream {
            Body::Stream(frames(source, self.limits).map_err(Failure::reading)?)
        } else {
            Body::Json(Some(source))
        };
        Ok(Exchange {
            body,
            state,
            limits: self.limits,
            expected_id: operation.expected_id,
            done: false,
            cancel,
            deadline,
        })
    }
}
struct Operation {
    method: Method,
    url: Url,
    body: Option<Vec<u8>>,
    stream: bool,
    expected_id: Option<String>,
}
fn bounded_source(
    response: reqwest::Response,
    deadline: Instant,
    idle: Duration,
    cancel: CancellationToken,
) -> BoxStream<'static, Result<Bytes, Error>> {
    futures_util::stream::unfold(
        (response.bytes_stream().boxed(), false),
        move |(mut source, done)| {
            let cancel = cancel.clone();
            async move {
                if done {
                    return None;
                }
                let until = std::cmp::min(deadline, Instant::now() + idle);
                let result = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Some(Err(Error::Cancelled)),
                    result = tokio::time::timeout_at(until, source.next()) => match result {
                        Ok(Some(Ok(bytes))) => Some(Ok(bytes)),
                        Ok(Some(Err(_))) => Some(Err(Error::Transport)),
                        Ok(None) => None,
                        Err(_) => Some(Err(Error::Timeout)),
                    }
                };
                result.map(|result| {
                    let done = result.is_err();
                    (result, (source, done))
                })
            }
        },
    )
    .boxed()
}
enum Body {
    Stream(BoxStream<'static, Result<crate::Frame, Error>>),
    Json(Option<BoxStream<'static, Result<Bytes, Error>>>),
}
pub enum Observation {
    Event(Update),
    Snapshot(Option<Status>),
}
pub struct Exchange {
    body: Body,
    state: ResponseState,
    limits: Limits,
    expected_id: Option<String>,
    done: bool,
    cancel: CancellationToken,
    deadline: Instant,
}
fn live(cancel: &CancellationToken, deadline: Instant) -> Result<(), Failure> {
    if cancel.is_cancelled() {
        return Err(Failure::sent(FailureKind::Cancelled));
    }
    if Instant::now() >= deadline {
        return Err(Failure::sent(FailureKind::Timeout));
    }
    Ok(())
}
impl Exchange {
    /// Plaintext state must be encrypted before persistence by the host.
    pub fn state(&self) -> &ResponseState {
        &self.state
    }
    pub fn into_state(self) -> ResponseState {
        self.state
    }
    pub async fn next(&mut self) -> Result<Option<Observation>, Failure> {
        if self.done {
            return Ok(None);
        }
        let result = match live(&self.cancel, self.deadline) {
            Ok(()) => self.read_next().await,
            Err(error) => Err(error),
        };
        if result.is_err() {
            self.done = true;
        }
        result
    }
    async fn read_next(&mut self) -> Result<Option<Observation>, Failure> {
        match &mut self.body {
            Body::Stream(source) => loop {
                let Some(frame) = source.next().await else {
                    return Err(Failure::reading(Error::Truncated));
                };
                let frame = frame.map_err(Failure::reading)?;
                live(&self.cancel, self.deadline)?;
                let Some(data) = frame.data else {
                    continue;
                };
                if let Some(event) = &frame.event {
                    let value: serde_json::Value = serde_json::from_str(&data)
                        .map_err(|_| Failure::reading(Error::InvalidJson))?;
                    if value.get("type").and_then(serde_json::Value::as_str) != Some(event.as_str())
                    {
                        return Err(Failure::reading(Error::Protocol));
                    }
                }
                let update = self
                    .state
                    .apply_json(data.as_bytes())
                    .map_err(Failure::reading)?;
                if matches!(update, Update::Terminal { .. }) {
                    self.done = true;
                }
                return Ok(Some(Observation::Event(update)));
            },
            Body::Json(source) => {
                let mut source = source
                    .take()
                    .ok_or_else(|| Failure::reading(Error::Protocol))?;
                let mut body = Vec::new();
                while let Some(bytes) = source.next().await {
                    let bytes = bytes.map_err(Failure::reading)?;
                    if body
                        .len()
                        .checked_add(bytes.len())
                        .is_none_or(|size| size > self.limits.max_stream_bytes)
                    {
                        return Err(Failure::reading(Error::Limit));
                    }
                    body.extend_from_slice(&bytes);
                }
                if let Some(expected) = &self.expected_id {
                    let value: serde_json::Value = serde_json::from_slice(&body)
                        .map_err(|_| Failure::reading(Error::InvalidJson))?;
                    if value.get("id").and_then(serde_json::Value::as_str) != Some(expected) {
                        return Err(Failure::reading(Error::Protocol));
                    }
                }
                live(&self.cancel, self.deadline)?;
                let status = self.state.apply_snapshot(&body).map_err(Failure::reading)?;
                self.done = true;
                Ok(Some(Observation::Snapshot(status)))
            }
        }
    }
}
