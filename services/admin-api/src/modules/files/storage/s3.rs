use super::{
    BlobId, BlobSpec, BlobStream, ProbeObservation, PutResult, StorageError, VerifiedSource,
};
use crate::modules::files::probes::ProbeJournal;
use crate::{
    config::Config,
    modules::settings::{
        dto::{Credentials, Integration},
        network,
    },
};
use futures_util::StreamExt;
use reqwest::{Method, Response, header::HeaderMap};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
pub(super) struct S3Store {
    config: Arc<Config>,
    base: url::Url,
    region: String,
    secret: Credentials,
    conditional_writes: tokio::sync::OnceCell<()>,
    journal: ProbeJournal,
}
impl S3Store {
    pub fn new(
        config: Arc<Config>,
        public: Integration,
        secret: Option<Credentials>,
        journal: ProbeJournal,
    ) -> Result<Self, StorageError> {
        let Integration::S3 {
            endpoint,
            bucket,
            region,
            prefix,
            path_style,
        } = public
        else {
            return Err(StorageError::InvalidInput);
        };
        let Some(secret @ Credentials::S3 { .. }) = secret else {
            return Err(StorageError::InvalidInput);
        };
        let mut base = url::Url::parse(&endpoint).map_err(|_| StorageError::InvalidInput)?;
        if path_style {
            base.path_segments_mut()
                .map_err(|_| StorageError::InvalidInput)?
                .pop_if_empty()
                .push(&bucket);
        } else {
            let host = network::url_host(&base).map_err(|_| StorageError::InvalidInput)?;
            if host.parse::<std::net::IpAddr>().is_ok() {
                return Err(StorageError::InvalidInput);
            }
            base.set_host(Some(&format!("{bucket}.{host}")))
                .map_err(|_| StorageError::InvalidInput)?;
        }
        {
            let mut path = base
                .path_segments_mut()
                .map_err(|_| StorageError::InvalidInput)?;
            path.pop_if_empty();
            for part in prefix.split('/').filter(|part| !part.is_empty()) {
                path.push(part);
            }
            path.push("objects");
        }
        Ok(Self {
            config,
            base,
            region,
            secret,
            conditional_writes: tokio::sync::OnceCell::new(),
            journal,
        })
    }
    fn url(&self, id: BlobId) -> Result<url::Url, StorageError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| StorageError::InvalidInput)?
            .push(&id.to_string());
        Ok(url)
    }
    fn headers(
        &self,
        method: &Method,
        url: &url::Url,
        hash: &str,
        mut headers: HeaderMap,
    ) -> Result<HeaderMap, StorageError> {
        let Credentials::S3 {
            access_key_id,
            secret_access_key,
            session_token,
        } = &self.secret
        else {
            return Err(StorageError::InvalidInput);
        };
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
            "tri-admin-file-storage",
        )
        .into();
        let mut settings = SigningSettings::default();
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name("s3")
            .time(std::time::SystemTime::now())
            .settings(settings)
            .build()
            .map_err(|_| StorageError::ConfigurationUnavailable)?
            .into();
        let fields = headers
            .iter()
            .map(|(key, value)| {
                value
                    .to_str()
                    .map(|value| (key.as_str(), value))
                    .map_err(|_| StorageError::InvalidInput)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let request = SignableRequest::new(
            method.as_str(),
            url.as_str(),
            fields.into_iter(),
            SignableBody::Precomputed(hash.into()),
        )
        .map_err(|_| StorageError::ConfigurationUnavailable)?;
        let (instructions, _) = sign(request, &params)
            .map_err(|_| StorageError::ConfigurationUnavailable)?
            .into_parts();
        let mut request = axum::http::Request::builder()
            .method(method.as_str())
            .uri(url.as_str())
            .body(())
            .map_err(|_| StorageError::InvalidInput)?;
        *request.headers_mut() = headers;
        instructions.apply_to_request_http1x(&mut request);
        headers = request.headers().clone();
        for name in ["authorization", "x-amz-security-token"] {
            if let Some(value) = headers.get_mut(name) {
                value.set_sensitive(true)
            }
        }
        Ok(headers)
    }
    async fn client(&self, url: &url::Url) -> Result<reqwest::Client, StorageError> {
        network::http_client_with_timeout(url, &self.config, Duration::from_secs(30 * 60))
            .await
            .map_err(|code| {
                if code == "DESTINATION_BLOCKED" {
                    StorageError::DestinationBlocked
                } else {
                    StorageError::ConnectionFailed
                }
            })
    }
    async fn probe_request(
        &self,
        client: &reqwest::Client,
        method: Method,
        url: &url::Url,
        body: &[u8],
    ) -> Result<Response, StorageError> {
        use sha2::{Digest, Sha256};
        let mut headers = HeaderMap::new();
        if method == Method::PUT {
            headers.insert(
                "if-none-match",
                "*".parse().map_err(|_| StorageError::InvalidInput)?,
            );
        }
        let headers = self.headers(
            &method,
            url,
            &format!("{:x}", Sha256::digest(body)),
            headers,
        )?;
        let mut request = client.request(method.clone(), url.clone()).headers(headers);
        if method == Method::PUT {
            request = request.body(body.to_vec());
        }
        request
            .send()
            .await
            .map_err(|_| StorageError::ConnectionFailed)
    }
    fn probe_url(&self, id: uuid::Uuid) -> Result<url::Url, StorageError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| StorageError::InvalidInput)?
            .pop()
            .push("_tri-probes")
            .push(&id.to_string());
        Ok(url)
    }
    pub async fn observe_probe(&self, id: uuid::Uuid) -> Result<ProbeObservation, StorageError> {
        let url = self.probe_url(id)?;
        let client = network::http_client(&url, &self.config)
            .await
            .map_err(|_| StorageError::ConnectionFailed)?;
        let response = self.probe_request(&client, Method::GET, &url, &[]).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(ProbeObservation::Missing);
        }
        if !response.status().is_success() {
            return Err(status(&response));
        }
        let mut bytes = Vec::with_capacity(1);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| StorageError::ConnectionFailed)?;
            if bytes.len() + chunk.len() > 1 {
                return Ok(ProbeObservation::Other);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(if bytes.len() == 1 {
            ProbeObservation::Value(bytes[0])
        } else {
            ProbeObservation::Other
        })
    }
    pub async fn delete_probe(&self, id: uuid::Uuid) -> Result<(), StorageError> {
        let url = self.probe_url(id)?;
        let client = network::http_client(&url, &self.config)
            .await
            .map_err(|_| StorageError::ConnectionFailed)?;
        let response = self
            .probe_request(&client, Method::DELETE, &url, &[])
            .await?;
        if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(status(&response))
        }
    }
    async fn check_conditional_writes(&self) -> Result<(), StorageError> {
        let id = uuid::Uuid::new_v4();
        let url = self.probe_url(id)?;
        let client = network::http_client(&url, &self.config)
            .await
            .map_err(|_| StorageError::ConnectionFailed)?;
        self.journal.begin(id).await?;
        let mut uncertain = false;
        let mut no_object = false;
        let result = async {
            self.journal.advance(id, 0, 1).await?;
            let first = match self.probe_request(&client, Method::PUT, &url, &[0]).await {
                Ok(response) => response,
                Err(error) => {
                    uncertain = true;
                    return Err(error);
                }
            };
            if !first.status().is_success() {
                uncertain = first.status().is_server_error() || first.status().is_redirection();
                no_object = !uncertain;
                return Err(status(&first));
            }
            self.journal.advance(id, 1, 2).await?;
            let second = match self.probe_request(&client, Method::PUT, &url, &[1]).await {
                Ok(response) => response,
                Err(error) => {
                    uncertain = true;
                    return Err(error);
                }
            };
            uncertain = second.status().is_server_error() || second.status().is_redirection();
            if second.status() != reqwest::StatusCode::PRECONDITION_FAILED {
                return Err(StorageError::Unsupported);
            }
            self.journal.advance(id, 2, 3).await?;
            let checked = self.probe_request(&client, Method::GET, &url, &[]).await?;
            if !checked.status().is_success() {
                return Err(status(&checked));
            }
            let mut stream = checked.bytes_stream();
            let mut contents = Vec::with_capacity(1);
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| StorageError::ConnectionFailed)?;
                if contents.len() + chunk.len() > 1 {
                    return Err(StorageError::Unsupported);
                }
                contents.extend_from_slice(&chunk);
            }
            if contents != [0] {
                return Err(StorageError::Unsupported);
            }
            Ok(())
        }
        .await;
        let cleanup = if self.journal.settle(id, uncertain, no_object).await? {
            match self.delete_probe(id).await {
                Ok(()) => self.journal.done(id).await,
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        result?;
        cleanup
    }
    pub async fn put(
        &self,
        id: BlobId,
        mut source: VerifiedSource,
        cancel: CancellationToken,
    ) -> Result<PutResult, StorageError> {
        self.conditional_writes
            .get_or_try_init(|| self.check_conditional_writes())
            .await?;
        if cancel.is_cancelled() {
            return Err(StorageError::Cancelled);
        }
        let url = self.url(id)?;
        let client = self.client(&url).await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            "if-none-match",
            "*".parse().map_err(|_| StorageError::InvalidInput)?,
        );
        headers.insert(
            "content-length",
            source
                .spec
                .size
                .to_string()
                .parse()
                .map_err(|_| StorageError::InvalidInput)?,
        );
        headers.insert(
            "content-type",
            "application/octet-stream"
                .parse()
                .map_err(|_| StorageError::InvalidInput)?,
        );
        let headers = self.headers(&Method::PUT, &url, &source.spec.sha256, headers)?;
        let file = source
            .file
            .as_mut()
            .ok_or(StorageError::Io)?
            .try_clone()
            .await
            .map_err(|_| StorageError::Io)?;
        if cancel.is_cancelled() {
            return Err(StorageError::Cancelled);
        }
        let request = client.put(url).headers(headers).body(file).send();
        let response = tokio::select! {biased;_=cancel.cancelled()=>return Err(StorageError::OutcomeUnknown),result=request=>result.map_err(|_|StorageError::OutcomeUnknown)?};
        if response.status().is_success() {
            return Ok(PutResult::Created);
        }
        if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            let mut stream =
                self.read(id, &source.spec, cancel.clone())
                    .await
                    .map_err(|error| {
                        if error == StorageError::IntegrityMismatch {
                            StorageError::Conflict
                        } else {
                            error
                        }
                    })?;
            let mut count = 0u64;
            use sha2::{Digest, Sha256};
            let mut hash = Sha256::new();
            while let Some(chunk) = tokio::select! {biased;_=cancel.cancelled()=>return Err(StorageError::Cancelled),value=stream.next()=>value}
            {
                let chunk = chunk?;
                count = count
                    .checked_add(chunk.len() as u64)
                    .ok_or(StorageError::Conflict)?;
                if count > source.spec.size {
                    return Err(StorageError::Conflict);
                }
                hash.update(&chunk);
            }
            if count == source.spec.size && format!("{:x}", hash.finalize()) == source.spec.sha256 {
                return Ok(PutResult::AlreadyPresent);
            }
            return Err(StorageError::Conflict);
        }
        if response.status().is_server_error() || response.status().is_redirection() {
            return Err(StorageError::OutcomeUnknown);
        }
        Err(status(&response))
    }
    pub async fn read(
        &self,
        id: BlobId,
        spec: &BlobSpec,
        cancel: CancellationToken,
    ) -> Result<BlobStream, StorageError> {
        let url = self.url(id)?;
        let client = self.client(&url).await?;
        let headers = self.headers(&Method::GET, &url, EMPTY_HASH, HeaderMap::new())?;
        let request = client.get(url).headers(headers).send();
        let response = tokio::select! {biased;_=cancel.cancelled()=>return Err(StorageError::Cancelled),result=request=>result.map_err(|_|StorageError::ConnectionFailed)?};
        if !response.status().is_success() {
            return Err(status(&response));
        }
        if response
            .content_length()
            .is_some_and(|size| size != spec.size)
        {
            return Err(StorageError::IntegrityMismatch);
        }
        Ok(Box::pin(response.bytes_stream().map(|value| {
            value.map_err(|_| StorageError::ConnectionFailed)
        })))
    }
    pub async fn delete(&self, id: BlobId, cancel: CancellationToken) -> Result<(), StorageError> {
        let url = self.url(id)?;
        let client = self.client(&url).await?;
        let headers = self.headers(&Method::DELETE, &url, EMPTY_HASH, HeaderMap::new())?;
        if cancel.is_cancelled() {
            return Err(StorageError::Cancelled);
        }
        let request = client.delete(url).headers(headers).send();
        let response = tokio::select! {biased;_=cancel.cancelled()=>return Err(StorageError::OutcomeUnknown),result=request=>result.map_err(|_|StorageError::OutcomeUnknown)?};
        if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(status(&response))
        }
    }
}
const EMPTY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
fn status(response: &Response) -> StorageError {
    match response.status().as_u16() {
        401 | 403 => StorageError::AuthenticationRejected,
        404 => StorageError::NotFound,
        409 | 412 => StorageError::Conflict,
        400 | 405 | 501 => StorageError::Unsupported,
        _ => StorageError::ConnectionFailed,
    }
}
