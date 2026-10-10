#![forbid(unsafe_code)]

//! Isolated Chromium rendering for Ratatoskr: durable render commands in, owned `BlobRef` evidence
//! out.

use std::sync::Arc;

use async_nats::jetstream;
use extractor_blob_store::BlobStore;
use futures_util::StreamExt as _;
use render_job::{
    NetworkEvidence, RENDER_COMPLETED_SUBJECT, RENDER_FAILED_SUBJECT, RENDER_REQUESTED_SUBJECT,
    RenderCommand, RenderCompleted, RenderFailed, RenderFailureClass,
};
use tokio_util::sync::CancellationToken;

/// Shared fleet command stream that already carries every `cmd.*` subject.
pub const COMMAND_STREAM: &str = "ratatoskr_commands";
/// Default KV bucket marking completed render jobs.
pub const DEFAULT_COMPLETIONS_BUCKET: &str = "browser_worker_completions";
/// Shared fleet event stream that already carries every `evt.*` subject.
pub const EVENTS_STREAM: &str = "ratatoskr_events";

/// Worker settings resolved from the process environment.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WorkerSettings {
    /// NATS URL for the command and event bus.
    pub nats_url: String,
    /// Optional explicit Chromium executable path.
    pub chrome_bin: Option<String>,
    /// Content-addressed root owned by this worker.
    pub blobs_root: std::path::PathBuf,
    /// Durable consumer name.
    pub durable_name: String,
    /// KV bucket marking completed render jobs.
    pub completions_bucket: String,
    /// Terminal jobs this process handles before exiting for a supervisor restart.
    pub max_jobs_per_process: u32,
    /// File holding the deployment nkey seed; absent only on an unauthenticated development broker.
    pub nkey_seed_path: Option<std::path::PathBuf>,
    /// Creates the streams, durable and bucket this process uses; development brokers only.
    pub provision_topology: bool,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        Self {
            nats_url: "nats://127.0.0.1:4222".to_owned(),
            chrome_bin: None,
            blobs_root: std::path::PathBuf::new(),
            durable_name: "ratatoskr_browser_worker".to_owned(),
            completions_bucket: DEFAULT_COMPLETIONS_BUCKET.to_owned(),
            max_jobs_per_process: 500,
            nkey_seed_path: None,
            provision_topology: false,
        }
    }
}

impl WorkerSettings {
    /// Loads settings from `BROWSER_*` environment variables with contract-safe defaults.
    ///
    /// # Errors
    ///
    /// Returns a message when the environment cannot be extracted.
    pub fn load() -> Result<Self, String> {
        let settings = figment::Figment::new()
            .merge(figment::providers::Env::prefixed("BROWSER_"))
            .extract::<Self>()
            .map_err(|error| error.to_string())?;
        if settings.provision_topology && settings.nkey_seed_path.is_some() {
            return Err(
                "BROWSER_PROVISION_TOPOLOGY is for unauthenticated development brokers and must \
                 not be combined with BROWSER_NKEY_SEED_PATH"
                    .to_owned(),
            );
        }
        Ok(settings)
    }
}

/// Connects to the bus, authenticating with the deployment nkey when one is configured.
///
/// # Errors
///
/// Returns [`WorkerError`] when the seed file or the broker is unavailable.
pub async fn connect(settings: &WorkerSettings) -> Result<async_nats::Client, WorkerError> {
    match &settings.nkey_seed_path {
        Some(path) => {
            let seed = std::fs::read_to_string(path).map_err(infrastructure)?;
            async_nats::ConnectOptions::with_nkey(seed.trim().to_owned())
                .connect(&settings.nats_url)
                .await
                .map_err(infrastructure)
        }
        None => async_nats::connect(&settings.nats_url)
            .await
            .map_err(infrastructure),
    }
}

/// Why a render job could not complete.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// A terminal failure class carried to the failure event.
    #[error("render failed: {}", .0.as_str())]
    Failed(RenderFailureClass),
    /// Infrastructure failed; the delivery stays unacknowledged for redelivery.
    #[error("worker infrastructure failed: {0}")]
    Infrastructure(#[from] Box<dyn std::error::Error + Send + Sync>),
    /// Artifact storage failed; the delivery stays unacknowledged for redelivery.
    #[error("worker storage failed")]
    Storage(#[from] extractor_blob_store::BlobStoreError),
}

fn infrastructure<E>(error: E) -> WorkerError
where
    E: std::error::Error + Send + Sync + 'static,
{
    WorkerError::Infrastructure(Box::new(error))
}

/// What one completed rendering produced before publication.
#[derive(Debug, Clone)]
pub struct RenderOutcome {
    /// Rendered DOM bytes.
    pub dom: Vec<u8>,
    /// Final URL after all hops.
    pub final_url: String,
    /// Network-evidence summary.
    pub evidence: NetworkEvidence,
}

/// Executes one render job inside Chromium.
pub trait RenderExecutor: Send + Sync {
    /// Renders the command target under its budgets.
    fn render(
        &self,
        command: &RenderCommand,
    ) -> impl std::future::Future<Output = Result<RenderOutcome, WorkerError>> + Send;
}

mod executor;
pub use executor::{ChromiumExecutor, ExecutorError, NavigationPolicy};

/// Loads the shared command and event streams and creates the completions bucket.
///
/// This is the development provisioning path (`provision_topology`); production processes never
/// call it, because Edge provisions the streams, the durable and the bucket. Both streams belong to
/// the fleet's capture pipeline and must already exist: a render-scoped stream created here would
/// silently narrow the subject set for every later publisher.
///
/// # Errors
///
/// Returns [`WorkerError`] when `JetStream` setup fails.
pub async fn ensure_render_stream(
    context: &jetstream::Context,
    completions_bucket: &str,
) -> Result<(), WorkerError> {
    let _ = context
        .get_stream(COMMAND_STREAM)
        .await
        .map_err(infrastructure)?;
    let _ = context.get_stream(EVENTS_STREAM).await.map_err(|error| {
        infrastructure(std::io::Error::other(format!(
            "the shared event stream must already exist (a development extractor creates it, Edge provisions it in production): {error}"
        )))
    })?;
    let _ = context
        .create_key_value(jetstream::kv::Config {
            bucket: completions_bucket.to_owned(),
            max_age: std::time::Duration::from_hours(24),
            ..jetstream::kv::Config::default()
        })
        .await
        .map_err(infrastructure)?;
    Ok(())
}

/// Consumes render commands until cancellation, executing each through `executor`.
///
/// # Errors
///
/// Returns [`WorkerError`] only when transport setup fails or an infrastructure error repeats;
/// per-job failures publish failure events and acknowledge the delivery.
pub async fn run_render_consumer<E>(
    context: jetstream::Context,
    settings: WorkerSettings,
    executor: E,
    cancellation: CancellationToken,
) -> Result<(), WorkerError>
where
    E: RenderExecutor,
{
    let consumer = if settings.provision_topology {
        ensure_render_stream(&context, &settings.completions_bucket).await?;
        context
            .get_stream(COMMAND_STREAM)
            .await
            .map_err(infrastructure)?
            .get_or_create_consumer(
                settings.durable_name.as_str(),
                render_consumer_config(&settings),
            )
            .await
            .map_err(infrastructure)?
    } else {
        verified_consumer(&context, &settings).await?
    };
    let store =
        Arc::new(BlobStore::new(&settings.blobs_root).with_owner("ratatoskr-browser-worker")?);
    let completions = context
        .get_key_value(&settings.completions_bucket)
        .await
        .map_err(|error| {
            topology_error(&format!(
                "the completions bucket `{}` is missing or unreachable ({error})",
                settings.completions_bucket
            ))
        })?;
    let mut messages = consumer.messages().await.map_err(infrastructure)?;
    let context = &context;
    let mut handled: u32 = 0;
    loop {
        let delivery = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok(()),
            next = messages.next() => next,
        };
        let Some(message) = delivery else {
            break;
        };
        match message {
            Ok(message) => {
                if handle_delivery(&message, context, &executor, &store, &completions).await
                    == DeliveryOutcome::Terminal
                {
                    handled += 1;
                    if handled >= settings.max_jobs_per_process {
                        tracing::info!(handled, "job budget reached; recycling the process");
                        return Ok(());
                    }
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "render delivery failed");
            }
        }
    }
    Ok(())
}

/// The durable's specification: Edge provisions exactly this (XR-021 CONTRACTS.md S04).
fn render_consumer_config(settings: &WorkerSettings) -> jetstream::consumer::pull::Config {
    jetstream::consumer::pull::Config {
        durable_name: Some(settings.durable_name.clone()),
        filter_subject: RENDER_REQUESTED_SUBJECT.to_owned(),
        ack_policy: jetstream::consumer::AckPolicy::Explicit,
        ack_wait: std::time::Duration::from_mins(5),
        max_deliver: 12,
        ..jetstream::consumer::pull::Config::default()
    }
}

fn topology_error(detail: &str) -> WorkerError {
    infrastructure(std::io::Error::other(format!(
        "{detail}; start ratatoskr-edge first so it provisions the topology"
    )))
}

/// Fetches the Edge-provisioned durable and verifies it against its specification.
async fn verified_consumer(
    context: &jetstream::Context,
    settings: &WorkerSettings,
) -> Result<jetstream::consumer::Consumer<jetstream::consumer::pull::Config>, WorkerError> {
    let missing = || {
        topology_error(&format!(
            "the durable `{}` on stream `{COMMAND_STREAM}` is missing, unreachable, not permitted or differs from its specification",
            settings.durable_name
        ))
    };
    let consumer = context
        .get_consumer_from_stream::<jetstream::consumer::pull::Config, _, _>(
            settings.durable_name.as_str(),
            COMMAND_STREAM,
        )
        .await
        .map_err(|_| missing())?;
    let actual = &consumer.cached_info().config;
    let expected = render_consumer_config(settings);
    let matches = actual.durable_name == expected.durable_name
        && actual.filter_subject == expected.filter_subject
        && actual.ack_policy == expected.ack_policy
        && actual.ack_wait == expected.ack_wait
        && actual.max_deliver == expected.max_deliver;
    if matches {
        Ok(consumer)
    } else {
        Err(missing())
    }
}

/// Whether one delivery reached a terminal outcome that counts against the
/// process job budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryOutcome {
    /// The executor ran; the job completed or failed terminally.
    Terminal,
    /// The delivery was malformed or already deduplicated; no render happened.
    Skipped,
}

/// Processes one delivery: render, publish evidence, mark, and acknowledge.
async fn handle_delivery<E>(
    message: &jetstream::Message,
    context: &jetstream::Context,
    executor: &E,
    store: &BlobStore,
    completions: &jetstream::kv::Store,
) -> DeliveryOutcome
where
    E: RenderExecutor,
{
    let command: RenderCommand = match serde_json::from_slice(&message.payload) {
        Ok(command) => command,
        Err(error) => {
            tracing::warn!(error = %error, "render command is malformed");
            message.ack().await.ok();
            return DeliveryOutcome::Skipped;
        }
    };
    if matches!(
        completions.get(command.render_id.to_string()).await,
        Ok(Some(_))
    ) {
        message.ack().await.ok();
        return DeliveryOutcome::Skipped;
    }
    match executor.render(&command).await {
        Ok(outcome) => {
            let blob = match store
                .store(
                    "text/html",
                    futures_util::stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from(
                        outcome.dom,
                    ))]),
                )
                .await
            {
                Ok(blob) => blob,
                Err(error) => {
                    tracing::warn!(error = %error, "rendered DOM persistence failed");
                    return DeliveryOutcome::Skipped;
                }
            };
            let completed = RenderCompleted {
                render_id: command.render_id,
                final_url: outcome.final_url,
                dom: blob,
                evidence: outcome.evidence,
            };
            if publish_event(context, RENDER_COMPLETED_SUBJECT, &completed)
                .await
                .is_err()
            {
                return DeliveryOutcome::Skipped;
            }
        }
        Err(WorkerError::Failed(class)) => {
            let failed = RenderFailed {
                render_id: command.render_id,
                class,
            };
            if publish_event(context, RENDER_FAILED_SUBJECT, &failed)
                .await
                .is_err()
            {
                return DeliveryOutcome::Skipped;
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, "worker infrastructure failed; leaving unacked");
            return DeliveryOutcome::Skipped;
        }
    }
    completions
        .put(command.render_id.to_string(), "done".into())
        .await
        .ok();
    message.ack().await.ok();
    DeliveryOutcome::Terminal
}

async fn publish_event<T: serde::Serialize>(
    context: &jetstream::Context,
    subject: &'static str,
    event: &T,
) -> Result<(), WorkerError> {
    let payload = serde_json::to_vec(event).map_err(infrastructure)?;
    let acknowledgement = context
        .publish(subject, payload.into())
        .await
        .map_err(infrastructure)?;
    acknowledgement.await.map_err(infrastructure)?;
    Ok(())
}
