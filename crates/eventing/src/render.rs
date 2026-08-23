//! Extractor-side render requests to the browser worker.

use async_nats::jetstream;
use futures_util::StreamExt as _;
use render_job::{
    RENDER_COMPLETED_SUBJECT, RENDER_FAILED_SUBJECT, RENDER_REQUESTED_SUBJECT, RenderCommand,
    RenderCompleted, RenderFailed,
};

/// Outcome of one awaited render job.
#[derive(Debug, Clone)]
pub enum RenderOutcome {
    /// The worker rendered the page; evidence announces worker-owned bytes.
    Completed(Box<RenderCompleted>),
    /// The worker failed the job with a stable class.
    Failed(RenderFailed),
}

/// Why a render request could not be made or awaited.
#[derive(Debug, thiserror::Error)]
pub enum RenderRequestError {
    /// JetStream transport failed; the command may or may not have been delivered.
    #[error("render request transport failed")]
    Transport(#[from] Box<dyn std::error::Error + Send + Sync>),
    /// The result did not arrive within the render budget.
    #[error("render result timed out")]
    Timeout,
}

fn infrastructure<E>(error: E) -> RenderRequestError
where
    E: std::error::Error + Send + Sync + 'static,
{
    RenderRequestError::Transport(Box::new(error))
}

/// Publishes one render command and awaits its completion or failure event.
///
/// # Errors
///
/// Returns [`RenderRequestError`] on transport failure or when the budget elapses before a
/// matching event arrives.
pub async fn request_render(
    context: &jetstream::Context,
    command: &RenderCommand,
) -> Result<RenderOutcome, RenderRequestError> {
    let payload = serde_json::to_vec(command).map_err(infrastructure)?;
    let acknowledgement = context
        .publish(RENDER_REQUESTED_SUBJECT, payload.into())
        .await
        .map_err(infrastructure)?;
    acknowledgement.await.map_err(infrastructure)?;

    let stream = context
        .get_stream(crate::EVENTS_STREAM)
        .await
        .map_err(infrastructure)?;
    let consumer = stream
        .get_or_create_consumer(
            "ratatoskr_extractor_render_awaits",
            jetstream::consumer::pull::Config {
                durable_name: None,
                filter_subject: "evt.content.render.>".to_owned(),
                ack_policy: jetstream::consumer::AckPolicy::None,
                inactive_threshold: std::time::Duration::from_secs(120),
                ..jetstream::consumer::pull::Config::default()
            },
        )
        .await
        .map_err(infrastructure)?;
    let mut messages = consumer.messages().await.map_err(infrastructure)?;

    let total = std::time::Duration::from_millis(command.budgets.total_timeout_ms);
    let deadline = tokio::time::Instant::now() + total;
    loop {
        let next = match tokio::time::timeout_at(deadline, messages.next()).await {
            Ok(next) => next,
            Err(_) => return Err(RenderRequestError::Timeout),
        };
        let Some(message) = next else {
            return Err(RenderRequestError::Timeout);
        };
        let message = match message {
            Ok(message) => message,
            Err(error) => return Err(infrastructure(error)),
        };
        if message.subject == RENDER_COMPLETED_SUBJECT.into() {
            if let Ok(completed) = serde_json::from_slice::<RenderCompleted>(&message.payload)
                && completed.render_id == command.render_id
            {
                return Ok(RenderOutcome::Completed(Box::new(completed)));
            }
        }
        if message.subject == RENDER_FAILED_SUBJECT.into()
            && let Ok(failed) = serde_json::from_slice::<RenderFailed>(&message.payload)
            && failed.render_id == command.render_id
        {
            return Ok(RenderOutcome::Failed(failed));
        }
    }
    Err(RenderRequestError::Timeout)
}
