#![forbid(unsafe_code)]

//! Safe observability for Ratatoskr Extractor.

use std::time::Duration;

use extractor_core::{LogFormat, OtlpConfig, TelemetryConfig};
use metrics_exporter_prometheus::{BuildError, PrometheusBuilder, PrometheusHandle};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::tonic_types::transport::ClientTlsConfig;
use opentelemetry_otlp::{SpanExporter, WithExportConfig as _, WithTonicConfig as _};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

const FETCH_FAILURES_TOTAL: &str = "ratatoskr_extractor_fetch_failures_total";
const FETCH_DURATION_SECONDS: &str = "ratatoskr_extractor_fetch_duration_seconds";
const FETCH_BYTES_TOTAL: &str = "ratatoskr_extractor_fetch_bytes_total";
const DURATION_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 10.0, 30.0,
];

/// Bounded fetch failure categories safe for logs and metric labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchFailureClass {
    /// URL or resolved address was denied by network policy.
    PolicyDenied,
    /// DNS lookup failed without a policy decision.
    Dns,
    /// Remote transport failed.
    Transport,
    /// A configured time budget expired.
    Timeout,
    /// A response exceeded a resource limit.
    ResourceLimit,
    /// Cache evidence did not match a stored artifact.
    CacheIntegrity,
    /// Local artifact persistence failed.
    Artifact,
}

/// Emits one bounded fetch failure observation.
pub fn record_fetch_failure(class: FetchFailureClass) {
    tracing::warn!(failure_class = class.as_str(), "fetch_failure");
    metrics::counter!(FETCH_FAILURES_TOTAL, "failure_class" => class.as_str()).increment(1);
}

/// Owns the process telemetry providers and metrics renderer.
#[derive(Debug)]
pub struct TelemetryGuard {
    provider: SdkTracerProvider,
    metrics: PrometheusHandle,
}

/// Why process telemetry could not be installed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TelemetryError {
    /// The configured log filter is invalid.
    #[error("the log filter is not a valid tracing directive")]
    Filter(#[source] tracing_subscriber::filter::ParseError),
    /// An exporter could not be built.
    #[error("a telemetry exporter could not be constructed")]
    Exporter(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// A process-global subscriber or recorder already exists.
    #[error("process telemetry is already installed")]
    AlreadyInstalled,
}

impl FetchFailureClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyDenied => "policy_denied",
            Self::Dns => "dns",
            Self::Transport => "transport",
            Self::Timeout => "timeout",
            Self::ResourceLimit => "resource_limit",
            Self::CacheIntegrity => "cache_integrity",
            Self::Artifact => "artifact",
        }
    }
}

impl TelemetryGuard {
    /// Returns a renderer for the admin `/metrics` route.
    #[must_use]
    pub fn metrics_handle(&self) -> PrometheusHandle {
        self.metrics.clone()
    }

    /// Flushes and closes the trace provider.
    pub fn shutdown(self) {
        if self.provider.shutdown().is_err() {
            tracing::warn!(failure_class = "telemetry_shutdown", "telemetry_failure");
        }
    }
}

/// Installs tracing, optional OTLP export, and Prometheus recording once.
///
/// Call this from a Tokio runtime when OTLP is configured.
///
/// # Errors
///
/// Returns [`TelemetryError`] for an invalid filter, exporter failure, or a second installation.
pub fn init(config: &TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    let filter = EnvFilter::try_new(&config.log_filter).map_err(TelemetryError::Filter)?;
    let provider = tracer_provider(config)?;
    let metrics = install_recorder()?;
    let layers = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("ratatoskr-extractor")));
    let installed = match config.log_format {
        LogFormat::Json => layers
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init(),
        LogFormat::Pretty => layers
            .with(tracing_subscriber::fmt::layer().pretty())
            .try_init(),
    };
    installed.map_err(|_| TelemetryError::AlreadyInstalled)?;
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    metrics::describe_counter!(FETCH_FAILURES_TOTAL, "Fetch failures by bounded class");
    metrics::describe_histogram!(FETCH_DURATION_SECONDS, "Safe fetch duration in seconds");
    metrics::describe_counter!(FETCH_BYTES_TOTAL, "Fetched bytes by representation");
    metrics::describe_counter!(
        "ratatoskr_extractor_commands_total",
        "Capture command outcomes"
    );
    metrics::describe_counter!(
        "ratatoskr_extractor_outbox_publications_total",
        "Outbox publication outcomes"
    );
    metrics::describe_counter!("ratatoskr_extractor_runs_total", "Extraction run outcomes");
    metrics::describe_histogram!(
        "ratatoskr_extractor_parse_duration_seconds",
        "Parse-once duration in seconds"
    );
    Ok(TelemetryGuard { provider, metrics })
}

fn tracer_provider(config: &TelemetryConfig) -> Result<SdkTracerProvider, TelemetryError> {
    let builder = SdkTracerProvider::builder().with_sampler(Sampler::AlwaysOn);
    match config.otlp.as_ref() {
        Some(otlp) => Ok(builder.with_batch_exporter(span_exporter(otlp)?).build()),
        None => Ok(builder.build()),
    }
}

fn span_exporter(config: &OtlpConfig) -> Result<SpanExporter, TelemetryError> {
    let mut builder = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(config.endpoint.as_str())
        .with_timeout(Duration::from_secs(config.timeout_seconds));
    if config.endpoint.scheme() == "https" {
        builder = builder.with_tls_config(ClientTlsConfig::new().with_enabled_roots());
    }
    builder
        .build()
        .map_err(|error| TelemetryError::Exporter(Box::new(error)))
}

fn install_recorder() -> Result<PrometheusHandle, TelemetryError> {
    PrometheusBuilder::new()
        .set_buckets(&DURATION_BUCKETS)
        .and_then(PrometheusBuilder::install_recorder)
        .map_err(|error| match error {
            BuildError::FailedToSetGlobalRecorder(_) => TelemetryError::AlreadyInstalled,
            other => TelemetryError::Exporter(Box::new(other)),
        })
}
