#![forbid(unsafe_code)]

//! Deterministic local fixtures for Ratatoskr Extractor tests.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http::{HeaderMap, StatusCode};
use http_body_util::StreamBody;
use hyper::body::Frame;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::sync::{Mutex, Notify};
use tokio::task::{JoinHandle, JoinSet};

/// One deterministic local HTTP response.
#[derive(Debug, Clone)]
pub struct ScriptedResponse {
    status: StatusCode,
    headers: HeaderMap,
    chunks: Vec<Bytes>,
    gate: Option<Arc<Notify>>,
}

/// One request observed by a [`ScriptedServer`].
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// Request method.
    pub method: http::Method,
    /// Origin-form request path and query.
    pub path_and_query: String,
    /// Request headers.
    pub headers: HeaderMap,
}

/// Ephemeral local HTTP server driven by a response queue.
#[derive(Debug)]
pub struct ScriptedServer {
    address: SocketAddr,
    request_count: Arc<AtomicUsize>,
    request_notify: Arc<Notify>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: JoinHandle<()>,
}

/// One deterministic DNS outcome.
#[derive(Debug, Clone)]
pub enum ScriptedResolution {
    /// A complete DNS answer set.
    Addresses(Vec<IpAddr>),
    /// A transport-level DNS failure.
    Failure,
}

/// Queue-backed resolver for DNS policy tests.
#[derive(Debug, Clone)]
pub struct ScriptedResolver {
    answers: Arc<Mutex<std::collections::VecDeque<ScriptedResolution>>>,
    calls: Arc<AtomicUsize>,
}

/// Why a scripted DNS lookup failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScriptedDnsError {
    /// The scripted answer is a DNS failure.
    #[error("the scripted DNS lookup failed")]
    Failure,
    /// No scripted answer remains.
    #[error("the scripted DNS answer queue is empty")]
    Exhausted,
}

/// Queue-backed deterministic retry jitter.
#[derive(Debug, Clone)]
pub struct JitterSequence {
    values: Arc<Mutex<std::collections::VecDeque<Duration>>>,
}

/// Extractor blob root deleted when its test owner drops.
#[derive(Debug)]
pub struct TemporaryBlobRoot {
    path: PathBuf,
}

impl ScriptedResponse {
    /// Returns status 200 with the supplied response chunks.
    pub fn chunks(chunks: impl IntoIterator<Item = Bytes>) -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            chunks: chunks.into_iter().collect(),
            gate: None,
        }
    }

    /// Returns a redirect to `location`.
    pub fn redirect(location: &str) -> Self {
        let mut headers = HeaderMap::new();
        if let Ok(value) = http::HeaderValue::from_str(location) {
            headers.insert(http::header::LOCATION, value);
        }
        Self {
            status: StatusCode::FOUND,
            headers,
            chunks: Vec::new(),
            gate: None,
        }
    }

    /// Adds one deterministic response header.
    #[must_use]
    pub fn with_header(mut self, name: http::HeaderName, value: http::HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Replaces the response status.
    #[must_use]
    pub const fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Holds this response until `gate` is notified.
    #[must_use]
    pub fn stall_until(mut self, gate: Arc<Notify>) -> Self {
        self.gate = Some(gate);
        self
    }
}

impl ScriptedServer {
    /// Starts a server on an ephemeral loopback port.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the ephemeral listener cannot bind.
    pub async fn start(scripts: Vec<ScriptedResponse>) -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let request_count = Arc::new(AtomicUsize::new(0));
        let request_notify = Arc::new(Notify::new());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let scripts = Arc::new(Mutex::new(std::collections::VecDeque::from(scripts)));
        let task = tokio::spawn(run_server(
            listener,
            Arc::clone(&scripts),
            Arc::clone(&request_count),
            Arc::clone(&request_notify),
            Arc::clone(&requests),
        ));
        Ok(Self {
            address,
            request_count,
            request_notify,
            requests,
            task,
        })
    }

    /// Builds a URL for a local request path.
    #[must_use]
    pub fn uri(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    /// Returns the ephemeral listener port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.address.port()
    }

    /// Returns the number of accepted requests.
    #[must_use]
    pub fn request_count(&self) -> usize {
        // Ordering: request recording is observed only after the atomic increment.
        self.request_count.load(Ordering::Acquire)
    }

    /// Waits until at least `count` requests have been recorded.
    pub async fn wait_for_requests(&self, count: usize) {
        loop {
            let notified = self.request_notify.notified();
            tokio::pin!(notified);
            if self.request_count() >= count {
                return;
            }
            notified.await;
        }
    }

    /// Returns a snapshot of recorded requests.
    pub async fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().await.clone()
    }
}

impl ScriptedResolver {
    /// Creates a resolver from ordered DNS outcomes.
    #[must_use]
    pub fn new(answers: impl IntoIterator<Item = ScriptedResolution>) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers.into_iter().collect())),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns the next scripted socket-address set.
    ///
    /// # Errors
    ///
    /// Returns [`ScriptedDnsError`] for a scripted failure or exhausted queue.
    pub async fn resolve(&self, port: u16) -> Result<Vec<SocketAddr>, ScriptedDnsError> {
        // Ordering: publish each call before the answer is consumed.
        self.calls.fetch_add(1, Ordering::Release);
        match self.answers.lock().await.pop_front() {
            Some(ScriptedResolution::Addresses(addresses)) => Ok(addresses
                .into_iter()
                .map(|address| SocketAddr::new(address, port))
                .collect()),
            Some(ScriptedResolution::Failure) => Err(ScriptedDnsError::Failure),
            None => Err(ScriptedDnsError::Exhausted),
        }
    }

    /// Returns the number of attempted resolutions.
    #[must_use]
    pub fn call_count(&self) -> usize {
        // Ordering: observe the published call count.
        self.calls.load(Ordering::Acquire)
    }
}

impl JitterSequence {
    /// Creates a deterministic jitter source.
    #[must_use]
    pub fn new(values: impl IntoIterator<Item = Duration>) -> Self {
        Self {
            values: Arc::new(Mutex::new(values.into_iter().collect())),
        }
    }

    /// Returns the next value, capped by the caller's backoff ceiling.
    pub async fn next(&self, ceiling: Duration) -> Duration {
        match self.values.lock().await.pop_front() {
            Some(value) => value.min(ceiling),
            None => Duration::ZERO,
        }
    }
}

impl TemporaryBlobRoot {
    /// Creates a unique empty blob root below the OS temporary directory.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the directory cannot be created.
    pub async fn create() -> Result<Self, std::io::Error> {
        let path =
            std::env::temp_dir().join(format!("ratatoskr-extractor-test-{}", uuid::Uuid::now_v7()));
        tokio::fs::create_dir_all(&path).await?;
        Ok(Self { path })
    }

    /// Returns the temporary root path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryBlobRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl Drop for ScriptedServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type ResponseBody =
    StreamBody<stream::Iter<std::vec::IntoIter<Result<Frame<Bytes>, std::convert::Infallible>>>>;

async fn run_server(
    listener: tokio::net::TcpListener,
    scripts: Arc<Mutex<std::collections::VecDeque<ScriptedResponse>>>,
    request_count: Arc<AtomicUsize>,
    request_notify: Arc<Notify>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            // cancel-safe: `join_next` removes a task only after completion.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            // cancel-safe: cancelling `accept` does not consume a connection.
            accepted = listener.accept() => match accepted {
                Ok((socket, _)) => {
                    let scripts = Arc::clone(&scripts);
                    let request_count = Arc::clone(&request_count);
                    let request_notify = Arc::clone(&request_notify);
                    let requests = Arc::clone(&requests);
                    connections.spawn(async move {
                        let service = service_fn(move |request| {
                            respond(
                                request,
                                Arc::clone(&scripts),
                                Arc::clone(&request_count),
                                Arc::clone(&request_notify),
                                Arc::clone(&requests),
                            )
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(socket), service)
                            .await;
                    });
                }
                Err(_) => return,
            }
        }
    }
}

async fn respond(
    request: hyper::Request<hyper::body::Incoming>,
    scripts: Arc<Mutex<std::collections::VecDeque<ScriptedResponse>>>,
    request_count: Arc<AtomicUsize>,
    request_notify: Arc<Notify>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
) -> Result<hyper::Response<ResponseBody>, std::convert::Infallible> {
    let script = match scripts.lock().await.pop_front() {
        Some(script) => script,
        None => ScriptedResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            headers: HeaderMap::new(),
            chunks: Vec::new(),
            gate: None,
        },
    };
    let recorded = RecordedRequest {
        method: request.method().clone(),
        path_and_query: request
            .uri()
            .path_and_query()
            .map_or_else(|| "/".to_owned(), ToString::to_string),
        headers: request.headers().clone(),
    };
    requests.lock().await.push(recorded);
    // Ordering: publish the recorded request before tests observe the count.
    request_count.fetch_add(1, Ordering::Release);
    #[expect(clippy::disallowed_methods, reason = "temporary diagnostic")]
    {
        eprintln!(
            "DIAG-SERVER {} -> status {} headers={:?}",
            request.uri(),
            script.status.as_u16(),
            request.headers()
        );
    }
    request_notify.notify_waiters();

    if let Some(gate) = script.gate {
        gate.notified().await;
    }
    let frames = script
        .chunks
        .into_iter()
        .map(|chunk| Ok(Frame::data(chunk)))
        .collect::<Vec<_>>();
    let mut response = hyper::Response::new(StreamBody::new(stream::iter(frames)));
    *response.status_mut() = script.status;
    *response.headers_mut() = script.headers;
    Ok(response)
}
