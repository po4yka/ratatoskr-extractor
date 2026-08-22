#![forbid(unsafe_code)]

//! Safe streaming fetch for Ratatoskr Extractor.

use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use extractor_blob_store::{BlobStore, BlobStoreError};
use extractor_core::FetchConfig;
use extractor_url_routing::{
    ResolutionError, RoutingPolicy, SystemDnsLookup, UrlError, ValidatingResolver, normalize,
    validate_address,
};
use futures_util::StreamExt as _;
use ratatoskr_identifiers::BlobRef;
use reqwest::redirect::Policy;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio_util::io::StreamReader;
use url::Url;

use crate::admission::Admission;
use crate::observations::record_success;
use crate::retry::{
    delay as retry_delay, eligible as retry_eligible, retry_after_at, sleep_before_deadline,
    transient_status,
};

#[cfg(any(test, feature = "test-support"))]
mod test_support;

mod admission;
mod observations;
mod retry;

/// One safe retrieval request.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// Untrusted absolute HTTP(S) URL.
    pub url: String,
    /// Optional validators tied to a prior verified artifact.
    pub prior: Option<CacheRecord>,
}

/// Cache validators that are meaningful only with their source bytes.
#[derive(Debug, Clone)]
pub struct CacheRecord {
    /// Prior extractor-owned bytes.
    pub artifact: BlobRef,
    /// Strong or weak entity validator.
    pub etag: Option<String>,
    /// HTTP date validator.
    pub last_modified: Option<String>,
    /// Prior cache policy evidence.
    pub cache_control: Option<String>,
}

/// Outcome of HTTP cache validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutcome {
    /// A response supplied new bytes.
    Fresh,
    /// A 304 response reused verified prior bytes.
    Revalidated,
}

/// Closed provenance and cache evidence for one successful fetch.
#[derive(Debug, Clone)]
pub struct FetchMetadata {
    /// Request before normalization.
    pub original_url: Url,
    /// Stable normalized request URL.
    pub normalized_url: Url,
    /// Validated redirect targets in order.
    pub redirects: Vec<Url>,
    /// Declared response media type, when valid.
    pub declared_media_type: Option<String>,
    /// Effective media type stored with the artifact.
    pub effective_media_type: String,
    /// Selected content encoding, when present.
    pub content_encoding: Option<String>,
    /// Returned `ETag` validator.
    pub etag: Option<String>,
    /// Returned Last-Modified validator.
    pub last_modified: Option<String>,
    /// Returned Cache-Control evidence.
    pub cache_control: Option<String>,
    /// Number of transport attempts.
    pub attempts: u32,
    /// Monotonic elapsed time for the complete operation.
    pub total_duration: Duration,
    /// Cache result.
    pub cache_outcome: CacheOutcome,
}

/// Evidence and raw artifact from one retrieval.
#[derive(Debug, Clone)]
pub struct FetchResult {
    /// Extractor-owned raw artifact.
    pub artifact: BlobRef,
    /// Final URL after validated redirects.
    pub final_url: Url,
    /// Final HTTP status code.
    pub status: u16,
    /// Encoded response bytes observed on the wire.
    pub wire_bytes: u64,
    /// Decoded source bytes stored in the artifact.
    pub decoded_bytes: u64,
    /// Effective media type of the stored bytes.
    pub media_type: String,
    /// Closed response and provenance evidence.
    pub metadata: FetchMetadata,
}

/// Safe retrieval failure classes.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SafeFetchError {
    /// Shared HTTP client construction failed.
    #[error("the safe HTTP client could not be constructed")]
    Client(#[source] reqwest::Error),
    /// URL validation failed before transport.
    #[error("the requested URL is not allowed")]
    Url(#[from] UrlError),
    /// An IP literal violates destination policy.
    #[error("the requested destination is prohibited")]
    AddressPolicy,
    /// Retrieval has not produced a response.
    #[error("safe retrieval is unavailable")]
    Unavailable,
    /// Remote HTTP transport failed.
    #[error("remote HTTP transport failed")]
    Transport,
    /// DNS failed or returned no usable destination.
    #[error("DNS resolution failed")]
    Dns,
    /// A completed non-success HTTP response.
    #[error("the remote server returned a non-success status")]
    RemoteStatus {
        /// Numeric status retained without response text or target URL.
        status: u16,
    },
    /// A redirect has no usable `Location` value.
    #[error("the redirect target is invalid")]
    RedirectLocation,
    /// Redirect history repeated a target.
    #[error("the redirect chain contains a loop")]
    RedirectLoop,
    /// Redirect count exceeded the configured bound.
    #[error("the redirect chain exceeded its limit")]
    RedirectLimit,
    /// The total operation deadline expired.
    #[error("the safe fetch total deadline expired")]
    TimeoutTotal,
    /// The first-byte phase deadline expired.
    #[error("the safe fetch first-byte deadline expired")]
    TimeoutFirstByte,
    /// The body stream remained idle past its phase limit.
    #[error("the safe fetch body read idle deadline expired")]
    TimeoutReadIdle,
    /// Declared response size exceeds the wire-byte limit.
    #[error("the declared response body exceeds the wire-byte limit")]
    DeclaredBodyTooLarge,
    /// Streamed response size exceeds the wire-byte limit.
    #[error("the response body exceeds the wire-byte limit")]
    WireBodyTooLarge,
    /// Decoded response size exceeds the decoded-byte limit.
    #[error("the decoded response body exceeds its limit")]
    DecodedBodyTooLarge,
    /// The server selected an encoding the extractor does not support.
    #[error("the response content encoding is unsupported")]
    UnsupportedEncoding,
    /// The selected content encoding is malformed.
    #[error("the response content encoding is malformed")]
    MalformedEncoding,
    /// A 304 response cannot be tied to verified prior bytes.
    #[error("cache validation could not verify the prior artifact")]
    CacheIntegrity,
    /// An in-flight capacity limit is already occupied.
    #[error("safe fetch capacity is unavailable")]
    Overloaded,
    /// Raw artifact persistence failed.
    #[error("the retrieved artifact could not be stored")]
    Artifact(#[from] BlobStoreError),
}

/// Shared bounded HTTP retriever.
#[derive(Debug, Clone)]
pub struct SafeFetcher {
    client: reqwest::Client,
    store: BlobStore,
    config: FetchConfig,
    admission: Admission,
    resolver: Option<ValidatingResolver<SystemDnsLookup>>,
}

impl FetchRequest {
    /// Creates a request for one untrusted URL.
    #[must_use]
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            prior: None,
        }
    }
}

impl SafeFetcher {
    /// Builds the shared Rustls client with a validating DNS resolver.
    ///
    /// # Errors
    ///
    /// Returns [`SafeFetchError`] when the client cannot be built.
    pub fn new(config: FetchConfig, blob_root: &Path) -> Result<Self, SafeFetchError> {
        let resolver = ValidatingResolver::new(SystemDnsLookup);
        let client = client_builder(&config)
            .dns_resolver(resolver.clone())
            .build()
            .map_err(SafeFetchError::Client)?;
        Ok(Self {
            client,
            store: BlobStore::new(blob_root),
            admission: Admission::new(config.global_concurrency, config.per_host_concurrency),
            resolver: Some(resolver),
            config,
        })
    }

    /// Fetches and stores one untrusted URL.
    ///
    /// # Errors
    ///
    /// Returns [`SafeFetchError`] for URL, network, limit, cache, or artifact failures.
    #[must_use]
    pub fn fetch(
        &self,
        request: FetchRequest,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<FetchResult, SafeFetchError>> + Send + '_>>
    {
        Box::pin(self.fetch_inner(request, true))
    }

    #[allow(
        clippy::too_many_lines,
        clippy::excessive_nesting,
        reason = "the bounded redirect and retry state machine is easier to audit in one sequence"
    )]
    async fn fetch_inner(
        &self,
        request: FetchRequest,
        enforce_literal_policy: bool,
    ) -> Result<FetchResult, SafeFetchError> {
        let started = tokio::time::Instant::now();
        let deadline = started + Duration::from_millis(self.config.total_timeout_ms);
        let policy = RoutingPolicy {
            max_url_length: self.config.max_url_length,
            allowed_ports: self.config.allowed_ports.clone(),
        };
        let mut normalized = normalize(&request.url, &policy)?;
        let prior = request.prior;
        let original_url = Url::parse(normalized.original()).map_err(UrlError::Invalid)?;
        let normalized_url = normalized.normalized().clone();
        let mut redirects = Vec::new();
        let trusted_test_origin = if enforce_literal_policy {
            None
        } else {
            Some(origin(&normalized))
        };
        if enforce_literal_policy {
            validate_literal(&normalized)?;
        }
        let host = normalized
            .normalized()
            .host_str()
            .ok_or(UrlError::MissingHost)?;
        let mut admission = self.admission.try_enter(host)?;
        let mut visited = HashSet::new();
        visited.insert(normalized.normalized().as_str().to_owned());
        let mut retries = 0_u16;
        let mut attempts = 0_u32;
        for hop in 0..=self.config.max_redirects {
            let response = loop {
                attempts = attempts.saturating_add(1);
                if let Some(resolver) = &self.resolver {
                    resolve_before_send(resolver, normalized.normalized(), deadline).await?;
                }
                let mut request_builder = self
                    .client
                    .get(normalized.normalized().clone())
                    .header(reqwest::header::ACCEPT_ENCODING, "gzip, br, zstd");
                if let Some(cache) = &prior {
                    if let Some(etag) = valid_header_value(cache.etag.as_ref()) {
                        request_builder =
                            request_builder.header(reqwest::header::IF_NONE_MATCH, etag);
                    }
                    if let Some(last_modified) = valid_header_value(cache.last_modified.as_ref()) {
                        request_builder = request_builder
                            .header(reqwest::header::IF_MODIFIED_SINCE, last_modified);
                    }
                }
                let send = request_builder.send();
                let first_byte_deadline = deadline.min(
                    tokio::time::Instant::now()
                        + Duration::from_millis(self.config.first_byte_timeout_ms),
                );
                let response = match tokio::time::timeout_at(first_byte_deadline, send).await {
                    Ok(Ok(response)) => response,
                    Ok(Err(_)) => {
                        let error = SafeFetchError::Transport;
                        if retry_eligible(&error) && retries < self.config.max_retries {
                            let delay = retry_delay(retries, attempts, None);
                            retries = retries.saturating_add(1);
                            sleep_before_deadline(deadline, delay).await?;
                            continue;
                        }
                        return Err(error);
                    }
                    Err(_) if first_byte_deadline == deadline => {
                        return Err(SafeFetchError::TimeoutTotal);
                    }
                    Err(_) => return Err(SafeFetchError::TimeoutFirstByte),
                };
                if transient_status(response.status()) && retries < self.config.max_retries {
                    let delay = retry_delay(
                        retries,
                        attempts,
                        retry_after_at(response.headers(), std::time::SystemTime::now()),
                    );
                    retries = retries.saturating_add(1);
                    sleep_before_deadline(deadline, delay).await?;
                    continue;
                }
                break response;
            };
            if is_redirect(response.status()) {
                if hop == self.config.max_redirects {
                    return Err(SafeFetchError::RedirectLimit);
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or(SafeFetchError::RedirectLocation)?;
                let next = normalized
                    .normalized()
                    .join(location)
                    .map_err(|_| SafeFetchError::RedirectLocation)?;
                normalized = normalize(next.as_str(), &policy)?;
                if trusted_test_origin.as_ref() != Some(&origin(&normalized)) {
                    validate_literal(&normalized)?;
                }
                let host = normalized
                    .normalized()
                    .host_str()
                    .ok_or(UrlError::MissingHost)?;
                drop(admission);
                admission = self.admission.try_enter(host)?;
                if !visited.insert(normalized.normalized().as_str().to_owned()) {
                    return Err(SafeFetchError::RedirectLoop);
                }
                redirects.push(normalized.normalized().clone());
                continue;
            }
            let context = ResponseContext {
                original_url,
                normalized_url,
                redirects,
                attempts,
                started,
            };
            if response.status() == reqwest::StatusCode::NOT_MODIFIED {
                let cache = prior.as_ref().ok_or(SafeFetchError::CacheIntegrity)?;
                return Box::pin(within_total_deadline(
                    deadline,
                    self.reuse_prior(response, context, cache),
                ))
                .await;
            }
            if !response.status().is_success() {
                return Err(SafeFetchError::RemoteStatus {
                    status: response.status().as_u16(),
                });
            }
            return Box::pin(within_total_deadline(
                deadline,
                self.store_response(response, context),
            ))
            .await;
        }
        Err(SafeFetchError::RedirectLimit)
    }

    #[allow(
        clippy::too_many_lines,
        clippy::excessive_nesting,
        reason = "wire and decoded counters must stay adjacent to the stream they guard"
    )]
    async fn store_response(
        &self,
        response: reqwest::Response,
        context: ResponseContext,
    ) -> Result<FetchResult, SafeFetchError> {
        if response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > self.config.max_wire_bytes)
        {
            return Err(SafeFetchError::DeclaredBodyTooLarge);
        }
        let status = response.status().as_u16();
        let final_url = response.url().clone();
        let declared_media_type = declared_media_type(response.headers());
        let declared_media_evidence =
            response_header(response.headers(), reqwest::header::CONTENT_TYPE);
        let content_encoding = content_encoding(response.headers())?;
        let content_encoding_evidence =
            response_header(response.headers(), reqwest::header::CONTENT_ENCODING);
        let etag = response_header(response.headers(), reqwest::header::ETAG);
        let last_modified = response_header(response.headers(), reqwest::header::LAST_MODIFIED);
        let cache_control = response_header(response.headers(), reqwest::header::CACHE_CONTROL);
        let wire_bytes = Arc::new(AtomicU64::new(0));
        let observed_wire_bytes = Arc::clone(&wire_bytes);
        let wire_limit_hit = Arc::new(AtomicBool::new(false));
        let observed_wire_limit = Arc::clone(&wire_limit_hit);
        let max_wire_bytes = self.config.max_wire_bytes;
        let stream = response.bytes_stream().map(move |result| match result {
            Ok(chunk) => {
                let Ok(length) = u64::try_from(chunk.len()) else {
                    return Err(std::io::Error::other("response chunk length overflow"));
                };
                // Ordering: this counter has no synchronization role; it is read after stream end.
                let previous = observed_wire_bytes.fetch_add(length, Ordering::Relaxed);
                if previous
                    .checked_add(length)
                    .is_none_or(|total| total > max_wire_bytes)
                {
                    // Ordering: the flag is read only after the stream returns its error.
                    observed_wire_limit.store(true, Ordering::Relaxed);
                    Err(std::io::Error::other("wire byte limit exceeded"))
                } else {
                    Ok(chunk)
                }
            }
            Err(_) => Err(std::io::Error::other("response body stream failed")),
        });
        let reader = StreamReader::new(stream);
        let mut reader = decoder(content_encoding, reader);
        let decoded_limit_hit = Arc::new(AtomicBool::new(false));
        let malformed_encoding = Arc::new(AtomicBool::new(false));
        let idle_timeout_hit = Arc::new(AtomicBool::new(false));
        let prefix_capacity = usize::try_from(self.config.max_decoded_bytes.min(511) + 1)
            .map_err(|_| SafeFetchError::DecodedBodyTooLarge)?;
        let mut prefix = vec![0_u8; prefix_capacity];
        let first_read_result = read_decoded(
            reader.as_mut(),
            &mut prefix,
            Duration::from_millis(self.config.read_idle_timeout_ms),
        )
        .await;
        if wire_limit_hit.load(Ordering::Relaxed) {
            return Err(SafeFetchError::WireBodyTooLarge);
        }
        let first_read = first_read_result.map_err(|kind| match kind {
            ReadFailure::Idle => SafeFetchError::TimeoutReadIdle,
            ReadFailure::Malformed => SafeFetchError::MalformedEncoding,
        })?;
        prefix.truncate(first_read);
        let first_read_u64 =
            u64::try_from(first_read).map_err(|_| SafeFetchError::DecodedBodyTooLarge)?;
        if first_read_u64 > self.config.max_decoded_bytes {
            return Err(SafeFetchError::DecodedBodyTooLarge);
        }
        let media_type = effective_media_type(&declared_media_type, &prefix);
        let decoded_limit = self.config.max_decoded_bytes;
        let idle_timeout = Duration::from_millis(self.config.read_idle_timeout_ms);
        let observed_decoded_limit = Arc::clone(&decoded_limit_hit);
        let observed_malformed = Arc::clone(&malformed_encoding);
        let observed_idle = Arc::clone(&idle_timeout_hit);
        let rest =
            futures_util::stream::unfold((reader, first_read_u64), move |(mut reader, decoded)| {
                let observed_decoded_limit = Arc::clone(&observed_decoded_limit);
                let observed_malformed = Arc::clone(&observed_malformed);
                let observed_idle = Arc::clone(&observed_idle);
                async move {
                    let mut buffer = vec![0_u8; 8_192];
                    match read_decoded(reader.as_mut(), &mut buffer, idle_timeout).await {
                        Ok(0) => None,
                        Ok(read) => {
                            let Ok(read_u64) = u64::try_from(read) else {
                                observed_decoded_limit.store(true, Ordering::Relaxed);
                                return Some((
                                    Err(std::io::Error::other("decoded limit")),
                                    (reader, decoded),
                                ));
                            };
                            let Some(next) = decoded.checked_add(read_u64) else {
                                observed_decoded_limit.store(true, Ordering::Relaxed);
                                return Some((
                                    Err(std::io::Error::other("decoded limit")),
                                    (reader, decoded),
                                ));
                            };
                            if next > decoded_limit {
                                observed_decoded_limit.store(true, Ordering::Relaxed);
                                return Some((
                                    Err(std::io::Error::other("decoded limit")),
                                    (reader, decoded),
                                ));
                            }
                            buffer.truncate(read);
                            Some((Ok(bytes::Bytes::from(buffer)), (reader, next)))
                        }
                        Err(ReadFailure::Idle) => {
                            observed_idle.store(true, Ordering::Relaxed);
                            Some((
                                Err(std::io::Error::other("body idle timeout")),
                                (reader, decoded),
                            ))
                        }
                        Err(ReadFailure::Malformed) => {
                            observed_malformed.store(true, Ordering::Relaxed);
                            Some((
                                Err(std::io::Error::other("malformed encoding")),
                                (reader, decoded),
                            ))
                        }
                    }
                }
            });
        let decoded_stream =
            futures_util::stream::once(async move { Ok(bytes::Bytes::from(prefix)) }).chain(rest);
        let artifact = match self
            .store
            .store(&media_type, Box::pin(decoded_stream))
            .await
        {
            Ok(artifact) => artifact,
            Err(_) if wire_limit_hit.load(Ordering::Relaxed) => {
                return Err(SafeFetchError::WireBodyTooLarge);
            }
            Err(_) if decoded_limit_hit.load(Ordering::Relaxed) => {
                return Err(SafeFetchError::DecodedBodyTooLarge);
            }
            Err(_) if malformed_encoding.load(Ordering::Relaxed) => {
                return Err(SafeFetchError::MalformedEncoding);
            }
            Err(_) if idle_timeout_hit.load(Ordering::Relaxed) => {
                return Err(SafeFetchError::TimeoutReadIdle);
            }
            Err(error) => return Err(SafeFetchError::Artifact(error)),
        };
        // Ordering: the body stream completed before this observation.
        let wire_bytes = wire_bytes.load(Ordering::Relaxed);
        let total_duration = context.started.elapsed();
        record_success("fresh", total_duration, wire_bytes, artifact.length_bytes);
        Ok(FetchResult {
            decoded_bytes: artifact.length_bytes,
            artifact,
            final_url,
            status,
            wire_bytes,
            media_type: media_type.clone(),
            metadata: FetchMetadata {
                original_url: context.original_url,
                normalized_url: context.normalized_url,
                redirects: context.redirects,
                declared_media_type: declared_media_evidence,
                effective_media_type: media_type.clone(),
                content_encoding: content_encoding_evidence,
                etag,
                last_modified,
                cache_control,
                attempts: context.attempts,
                total_duration,
                cache_outcome: CacheOutcome::Fresh,
            },
        })
    }

    async fn reuse_prior(
        &self,
        response: reqwest::Response,
        context: ResponseContext,
        prior: &CacheRecord,
    ) -> Result<FetchResult, SafeFetchError> {
        self.store
            .verify(&prior.artifact)
            .await
            .map_err(|_| SafeFetchError::CacheIntegrity)?;
        let final_url = response.url().clone();
        let etag = response_header(response.headers(), reqwest::header::ETAG)
            .or_else(|| prior.etag.clone());
        let last_modified = response_header(response.headers(), reqwest::header::LAST_MODIFIED)
            .or_else(|| prior.last_modified.clone());
        let cache_control = response_header(response.headers(), reqwest::header::CACHE_CONTROL)
            .or_else(|| prior.cache_control.clone());
        let media_type = prior.artifact.media_type.as_str().to_owned();
        let total_duration = context.started.elapsed();
        record_success(
            "revalidated",
            total_duration,
            0,
            prior.artifact.length_bytes,
        );
        Ok(FetchResult {
            artifact: prior.artifact.clone(),
            final_url,
            status: response.status().as_u16(),
            wire_bytes: 0,
            decoded_bytes: prior.artifact.length_bytes,
            media_type: media_type.clone(),
            metadata: FetchMetadata {
                original_url: context.original_url,
                normalized_url: context.normalized_url,
                redirects: context.redirects,
                declared_media_type: None,
                effective_media_type: media_type,
                content_encoding: None,
                etag,
                last_modified,
                cache_control,
                attempts: context.attempts,
                total_duration,
                cache_outcome: CacheOutcome::Revalidated,
            },
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum Encoding {
    Identity,
    Gzip,
    Brotli,
    Zstd,
}

#[derive(Debug)]
struct ResponseContext {
    original_url: Url,
    normalized_url: Url,
    redirects: Vec<Url>,
    attempts: u32,
    started: tokio::time::Instant,
}

#[derive(Debug, Clone, Copy)]
enum ReadFailure {
    Idle,
    Malformed,
}

fn content_encoding(headers: &reqwest::header::HeaderMap) -> Result<Encoding, SafeFetchError> {
    match headers
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("" | "identity") => Ok(Encoding::Identity),
        Some("gzip") => Ok(Encoding::Gzip),
        Some("br") => Ok(Encoding::Brotli),
        Some("zstd") => Ok(Encoding::Zstd),
        Some(_) => Err(SafeFetchError::UnsupportedEncoding),
    }
}

fn response_header(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn valid_header_value(value: Option<&String>) -> Option<reqwest::header::HeaderValue> {
    value
        .map(String::as_str)
        .and_then(|value| reqwest::header::HeaderValue::from_str(value).ok())
}

fn decoder<R>(encoding: Encoding, reader: R) -> std::pin::Pin<Box<dyn AsyncRead + Send>>
where
    R: tokio::io::AsyncBufRead + Send + 'static,
{
    match encoding {
        Encoding::Identity => Box::pin(reader),
        Encoding::Gzip => Box::pin(async_compression::tokio::bufread::GzipDecoder::new(reader)),
        Encoding::Brotli => Box::pin(async_compression::tokio::bufread::BrotliDecoder::new(
            reader,
        )),
        Encoding::Zstd => Box::pin(async_compression::tokio::bufread::ZstdDecoder::new(reader)),
    }
}

async fn read_decoded(
    mut reader: std::pin::Pin<&mut (dyn AsyncRead + Send)>,
    buffer: &mut [u8],
    idle_timeout: Duration,
) -> Result<usize, ReadFailure> {
    match tokio::time::timeout(idle_timeout, reader.read(buffer)).await {
        Ok(Ok(read)) => Ok(read),
        Ok(Err(_)) => Err(ReadFailure::Malformed),
        Err(_) => Err(ReadFailure::Idle),
    }
}

fn effective_media_type(declared: &str, prefix: &[u8]) -> String {
    let trimmed = prefix
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect::<Vec<_>>();
    if prefix.starts_with(b"%PDF-") {
        "application/pdf".to_owned()
    } else if trimmed.starts_with(b"<!DOCTYPE html")
        || trimmed.starts_with(b"<!doctype html")
        || trimmed.starts_with(b"<html")
    {
        "text/html".to_owned()
    } else {
        declared.to_owned()
    }
}

async fn resolve_before_send(
    resolver: &ValidatingResolver<SystemDnsLookup>,
    url: &Url,
    deadline: tokio::time::Instant,
) -> Result<(), SafeFetchError> {
    let host = url.host_str().ok_or(UrlError::MissingHost)?;
    match tokio::time::timeout_at(deadline, resolver.resolve_host(host)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => match error {
            ResolutionError::Policy { .. } => Err(SafeFetchError::AddressPolicy),
            _ => Err(SafeFetchError::Dns),
        },
        Err(_) => Err(SafeFetchError::TimeoutTotal),
    }
}

fn validate_literal(url: &extractor_url_routing::NormalizedUrl) -> Result<(), SafeFetchError> {
    if let Some(host) = url.normalized().host() {
        match host {
            url::Host::Ipv4(address) => {
                validate_address(address.into()).map_err(|_| SafeFetchError::AddressPolicy)?;
            }
            url::Host::Ipv6(address) => {
                validate_address(address.into()).map_err(|_| SafeFetchError::AddressPolicy)?;
            }
            url::Host::Domain(_) => {}
        }
    }
    Ok(())
}

fn origin(url: &extractor_url_routing::NormalizedUrl) -> (String, Option<u16>) {
    (
        url.normalized()
            .host_str()
            .map_or_else(String::new, str::to_owned),
        url.normalized().port_or_known_default(),
    )
}

const fn is_redirect(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::MOVED_PERMANENTLY
            | reqwest::StatusCode::FOUND
            | reqwest::StatusCode::SEE_OTHER
            | reqwest::StatusCode::TEMPORARY_REDIRECT
            | reqwest::StatusCode::PERMANENT_REDIRECT
    )
}

fn client_builder(config: &FetchConfig) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_millis(config.connect_timeout_ms))
        .user_agent("Ratatoskr-Extractor/0.1")
}

/// cancel-safe when `future` is cancel-safe: timeout only drops the supplied future at deadline.
async fn within_total_deadline<T, F>(
    deadline: tokio::time::Instant,
    future: F,
) -> Result<T, SafeFetchError>
where
    F: Future<Output = Result<T, SafeFetchError>>,
{
    match tokio::time::timeout_at(deadline, future).await {
        Ok(result) => result,
        Err(_) => Err(SafeFetchError::TimeoutTotal),
    }
}

fn declared_media_type(headers: &reqwest::header::HeaderMap) -> String {
    let declared = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match declared {
        Some(value) => value.to_ascii_lowercase(),
        None => "application/octet-stream".to_owned(),
    }
}

#[cfg(test)]
mod tests;
