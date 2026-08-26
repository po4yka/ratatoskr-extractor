#![forbid(unsafe_code)]

//! Process composition for Ratatoskr Extractor.

mod escalation;
mod pipeline;
mod provider_continuation;
mod youtube_media;
pub mod youtube_pipeline;

pub use pipeline::{
    ProcessError, complete_provider_for_test as _complete_provider_for_test, process_run,
};
pub use youtube_media::{
    ArchivalOutcome, DownloadedMedia, MediaArchiver, MediaDownloadError, MediaDownloader,
    YtDlpDownloader,
};
pub use youtube_pipeline::complete_youtube_for_test;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use tokio_util::sync::CancellationToken;

const STARTING: u8 = 0;
const READY: u8 = 1;
const DRAINING: u8 = 2;

/// Process lifecycle state observed by the admin listener.
#[derive(Debug, Clone)]
pub struct RuntimeHealth {
    state: Arc<AtomicU8>,
    dependencies_healthy: Arc<AtomicBool>,
}

#[derive(Clone)]
struct AdminState {
    health: RuntimeHealth,
    render_metrics: Arc<dyn Fn() -> String + Send + Sync>,
}

/// Shared admission gate for extractor operations.
#[derive(Debug, Clone)]
pub struct AdmissionController {
    state: Arc<AdmissionState>,
}

/// Ownership proof for one admitted operation.
#[derive(Debug)]
pub struct AdmissionPermit {
    state: Arc<AdmissionState>,
    cancellation: CancellationToken,
}

#[derive(Debug)]
struct AdmissionState {
    accepting: AtomicBool,
    active: AtomicUsize,
    cancellation: CancellationToken,
    drained: tokio::sync::Notify,
}

/// Coordinates readiness, admission, and the one shutdown deadline.
#[derive(Debug, Clone)]
pub struct ShutdownCoordinator {
    health: RuntimeHealth,
    admission: AdmissionController,
    grace: Duration,
}

/// A new operation was refused during drain.
#[derive(Debug, thiserror::Error)]
#[error("the extractor is draining")]
pub struct AdmissionError;

impl RuntimeHealth {
    /// Creates a process that is live but not ready.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(STARTING)),
            dependencies_healthy: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Marks startup as complete.
    pub fn mark_ready(&self) {
        self.dependencies_healthy.store(true, Ordering::Release);
        // Ordering: publish completed startup before readiness becomes observable.
        self.state.store(READY, Ordering::Release);
    }

    /// Updates live `PostgreSQL` and NATS readiness.
    pub fn mark_dependencies_healthy(&self, healthy: bool) {
        self.dependencies_healthy.store(healthy, Ordering::Release);
    }

    /// Starts drain and makes readiness fail.
    pub fn begin_drain(&self) {
        // Ordering: publish drain before new admission observes readiness.
        self.state.store(DRAINING, Ordering::Release);
    }

    fn is_ready(&self) -> bool {
        // Ordering: pair with startup and drain publication stores.
        self.state.load(Ordering::Acquire) == READY
            && self.dependencies_healthy.load(Ordering::Acquire)
    }
}

impl Default for RuntimeHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl AdmissionController {
    /// Creates an admission gate that accepts work.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(AdmissionState {
                accepting: AtomicBool::new(true),
                active: AtomicUsize::new(0),
                cancellation: CancellationToken::new(),
                drained: tokio::sync::Notify::new(),
            }),
        }
    }

    /// Attempts immediate admission without a queue.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError`] after shutdown starts.
    pub fn try_admit(&self) -> Result<AdmissionPermit, AdmissionError> {
        // Ordering: observe drain publication before adding active work.
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(AdmissionError);
        }
        // Ordering: publish the permit count before the drain path reads it.
        self.state.active.fetch_add(1, Ordering::AcqRel);
        // Ordering: close the race with drain beginning after the first load.
        if self.state.accepting.load(Ordering::Acquire) {
            Ok(AdmissionPermit {
                state: Arc::clone(&self.state),
                cancellation: self.state.cancellation.child_token(),
            })
        } else {
            self.release_one();
            Err(AdmissionError)
        }
    }

    fn close(&self) {
        // Ordering: every later admission must observe drain.
        self.state.accepting.store(false, Ordering::Release);
        // Ordering: pair with permit release before deciding the gate is drained.
        if self.state.active.load(Ordering::Acquire) == 0 {
            self.state.drained.notify_waiters();
        }
    }

    fn cancel(&self) {
        self.state.cancellation.cancel();
    }

    fn release_one(&self) {
        // Ordering: publish operation completion before waking shutdown.
        let previous = self.state.active.fetch_sub(1, Ordering::AcqRel);
        if previous == 1 {
            self.state.drained.notify_waiters();
        }
    }

    /// cancel-safe: cancellation only removes a notification waiter; the active count is unchanged.
    async fn drained(&self) {
        loop {
            let notified = self.state.drained.notified();
            tokio::pin!(notified);
            // Ordering: pair with every permit release before accepting a drained state.
            if self.state.active.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}

impl Default for AdmissionController {
    fn default() -> Self {
        Self::new()
    }
}

impl AdmissionPermit {
    /// Returns the cancellation signal for this operation.
    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let controller = AdmissionController {
            state: Arc::clone(&self.state),
        };
        controller.release_one();
    }
}

impl ShutdownCoordinator {
    /// Creates a coordinator with one grace deadline.
    #[must_use]
    pub const fn new(
        health: RuntimeHealth,
        admission: AdmissionController,
        grace: Duration,
    ) -> Self {
        Self {
            health,
            admission,
            grace,
        }
    }

    /// Starts drain and waits for the configured shutdown boundary.
    pub async fn shutdown(&self) {
        self.health.begin_drain();
        self.admission.close();
        let deadline = tokio::time::Instant::now() + self.grace;
        // NOT cancel-safe: dropping shutdown would leave the service draining without signalling
        // active operations; the process owner must spawn and join this future.
        if tokio::time::timeout_at(deadline, self.admission.drained())
            .await
            .is_err()
        {
            self.admission.cancel();
        }
    }
}

/// Builds the isolated operator router.
pub fn admin_router(
    health: RuntimeHealth,
    metrics_renderer: impl Fn() -> String + Send + Sync + 'static,
) -> Router {
    let state = AdminState {
        health,
        render_metrics: Arc::new(metrics_renderer),
    };
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/metrics", get(render_metrics))
        .route("/version", get(version))
        .with_state(state)
}

async fn live() -> impl IntoResponse {
    no_store(StatusCode::OK, "live")
}

async fn ready(axum::extract::State(state): axum::extract::State<AdminState>) -> impl IntoResponse {
    if state.health.is_ready() {
        no_store(StatusCode::OK, "ready")
    } else {
        no_store(StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

async fn render_metrics(
    axum::extract::State(state): axum::extract::State<AdminState>,
) -> impl IntoResponse {
    no_store(StatusCode::OK, (state.render_metrics)())
}

async fn version() -> impl IntoResponse {
    no_store(StatusCode::OK, env!("CARGO_PKG_VERSION"))
}

fn no_store(status: StatusCode, body: impl IntoResponse) -> impl IntoResponse {
    (status, [(header::CACHE_CONTROL, "no-store")], body)
}
