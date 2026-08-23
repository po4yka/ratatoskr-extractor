#![forbid(unsafe_code)]

//! Isolated Chromium rendering for Ratatoskr: durable render commands in, owned BlobRef evidence
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
#[derive(Debug, Clone)]
pub struct WorkerSettings {
    /// NATS URL for the command and event bus.
    pub nats_url: String,
    /// Content-addressed root owned by this worker.
    pub blobs_root: std::path::PathBuf,
    /// Durable consumer name.
    pub durable_name: String,
    /// KV bucket marking completed render jobs.
    pub completions_bucket: String,
}

impl WorkerSettings {
    /// Reads settings from the environment with contract-safe defaults.
    ///
    /// # Errors
    ///
    /// Returns the missing variable names joined by commas.
    pub fn from_env() -> Result<Self, String> {
        let blobs_root = match std::env::var("BROWSER_BLOBS_ROOT") {
            Ok(value) => value,
            Err(_) => return Err("BROWSER_BLOBS_ROOT".to_owned()),
        };
        Ok(Self {
            nats_url: std::env::var("BROWSER_NATS_URL")
                .unwrap_or_else(|_| "nats://127.0.0.1:4222".to_owned()),
            blobs_root: std::path::PathBuf::from(blobs_root),
            durable_name: "ratatoskr_browser_worker".to_owned(),
            completions_bucket: DEFAULT_COMPLETIONS_BUCKET.to_owned(),
        })
    }
}

/// Why a render job could not complete.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// A terminal failure class carried to the failure event.
    #[error("render failed: {}", .0.as_str())]
    Failed(RenderFailureClass),
    /// Infrastructure failed; the delivery stays unacknowledged for redelivery.
    #[error("worker infrastructure failed")]
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

fn already_boxed(error: Box<dyn std::error::Error + Send + Sync>) -> WorkerError {
    WorkerError::Infrastructure(error)
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

/// Loads the shared command stream and creates the completions bucket.
///
/// The command stream itself belongs to the fleet's capture pipeline and must already exist.
///
/// # Errors
///
/// Returns [`WorkerError`] when JetStream setup fails.
pub async fn ensure_render_stream(
    context: &jetstream::Context,
    completions_bucket: &str,
) -> Result<(), WorkerError> {
    let _ = context
        .get_stream(COMMAND_STREAM)
        .await
        .map_err(infrastructure)?;
    let _ = context
        .get_stream(EVENTS_STREAM)
        .await
        .map_err(infrastructure)?;
    let _ = context
        .create_key_value(jetstream::kv::Config {
            bucket: completions_bucket.to_owned(),
            max_age: std::time::Duration::from_secs(24 * 60 * 60),
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
    ensure_render_stream(&context, &settings.completions_bucket).await?;
    let stream = context
        .get_stream(COMMAND_STREAM)
        .await
        .map_err(infrastructure)?;
    let consumer = stream
        .get_or_create_consumer(
            settings.durable_name.as_str(),
            jetstream::consumer::pull::Config {
                durable_name: Some(settings.durable_name.clone()),
                filter_subject: RENDER_REQUESTED_SUBJECT.to_owned(),
                ack_policy: jetstream::consumer::AckPolicy::Explicit,
                ack_wait: std::time::Duration::from_secs(300),
                max_deliver: 12,
                ..jetstream::consumer::pull::Config::default()
            },
        )
        .await
        .map_err(|error| WorkerError::Infrastructure(Box::new(error)))?;
    let store =
        Arc::new(BlobStore::new(&settings.blobs_root).with_owner("ratatoskr-browser-worker")?);
    let completions = context
        .get_key_value(&settings.completions_bucket)
        .await
        .map_err(infrastructure)?;
    let mut messages = consumer.messages().await.map_err(infrastructure)?;
    loop {
        let message = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok(()),
            next = messages.next() => next,
        };
        let Some(message) = message else {
            break;
        };
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                tracing::warn!(error = %error, "render delivery failed");
                continue;
            }
        };
        let command: RenderCommand = match serde_json::from_slice(&message.payload) {
            Ok(command) => command,
            Err(error) => {
                tracing::warn!(error = %error, "render command is malformed");
                message.ack().await.map_err(already_boxed)?;
                continue;
            }
        };
        let dedup_state = completions.get(command.render_id.to_string()).await;
        if matches!(dedup_state, Ok(Some(_))) {
            message.ack().await.map_err(already_boxed)?;
            continue;
        }
        match executor.render(&command).await {
            Ok(outcome) => {
                let blob = store
                    .store(
                        "text/html",
                        futures_util::stream::iter([
                            Ok::<_, extractor_blob_store::BlobStoreError>(bytes::Bytes::from(
                                outcome.dom,
                            )),
                        ]),
                    )
                    .await?;
                eprintln!(
                    "DIAG-WORKER stored {:?}, publishing",
                    blob.digest.hex.as_str()
                );
                let completed = RenderCompleted {
                    render_id: command.render_id,
                    final_url: outcome.final_url,
                    dom: blob,
                    evidence: outcome.evidence,
                };
                publish_event(&context, RENDER_COMPLETED_SUBJECT, &completed).await?;
                completions
                    .put(command.render_id.to_string(), "completed".into())
                    .await
                    .map_err(infrastructure)?;
                message.ack().await.map_err(already_boxed)?;
            }
            Err(WorkerError::Failed(class)) => {
                let failed = RenderFailed {
                    render_id: command.render_id,
                    class,
                };
                publish_event(&context, RENDER_FAILED_SUBJECT, &failed).await?;
                completions
                    .put(command.render_id.to_string(), class.as_str().into())
                    .await
                    .map_err(infrastructure)?;
                message.ack().await.map_err(already_boxed)?;
            }
            Err(error @ (WorkerError::Infrastructure(_) | WorkerError::Storage(_))) => {
                tracing::warn!(error = %error, "worker infrastructure failed; leaving unacked");
                continue;
            }
        }
    }
    Ok(())
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
