//! Extractor-side render requests to the browser worker.

use async_nats::jetstream;
use futures_util::StreamExt as _;

use crate::ConsumeError;
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
    /// `JetStream` transport failed; the command may or may not have been delivered.
    #[error("render request transport failed")]
    Transport(#[from] Box<dyn std::error::Error + Send + Sync>),
    /// The result did not arrive within the render budget.
    #[error("render result timed out")]
    Timeout,
}

/// Outcome of one per-UTC-day render-budget slot attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderBudget {
    /// The slot was consumed; `count` is the day's escalation total after it.
    Consumed {
        /// The day counter value after consuming this slot.
        count: i32,
    },
    /// The configured maximum is already reached for this UTC day.
    Exhausted,
}

/// Consumes one slot of the durable per-UTC-day render budget atomically.
///
/// The seed and the guarded increment run inside one transaction; the `update`
/// takes the day row's lock, so concurrent runs serialise against the committed
/// counter and can never exceed the configured maximum.
///
/// # Errors
///
/// Returns [`ConsumeError`] when `PostgreSQL` access fails.
pub async fn consume_render_budget(
    pool: &sqlx::PgPool,
    max_escalations_per_day: u32,
) -> Result<RenderBudget, ConsumeError> {
    let cap = i32::try_from(max_escalations_per_day)
        .map_err(|_| ConsumeError::InvalidRunState)?
        .saturating_sub(1);
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "insert into extractor.render_budgets (utc_day, escalated)
         values (current_date, 0)
         on conflict (utc_day) do nothing",
    )
    .execute(&mut *transaction)
    .await?;
    let consumed = sqlx::query_scalar::<_, i32>(
        "update extractor.render_budgets
            set escalated = escalated + 1
          where utc_day = current_date
            and escalated <= $1
         returning escalated",
    )
    .bind(cap)
    .fetch_optional(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(
        consumed.map_or(RenderBudget::Exhausted, |count| RenderBudget::Consumed {
            count,
        }),
    )
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
                inactive_threshold: std::time::Duration::from_mins(2),
                ..jetstream::consumer::pull::Config::default()
            },
        )
        .await
        .map_err(infrastructure)?;
    let mut messages = consumer.messages().await.map_err(infrastructure)?;

    let total = std::time::Duration::from_millis(command.budgets.total_timeout_ms);
    let deadline = tokio::time::Instant::now() + total;
    loop {
        let Ok(next) = tokio::time::timeout_at(deadline, messages.next()).await else {
            return Err(RenderRequestError::Timeout);
        };
        let Some(Ok(message)) = next else {
            // A delivery error leaves the pull request; the deadline handles termination.
            return Err(RenderRequestError::Timeout);
        };
        if message.subject == RENDER_COMPLETED_SUBJECT.into()
            && let Ok(completed) = serde_json::from_slice::<RenderCompleted>(&message.payload)
            && completed.render_id == command.render_id
        {
            return Ok(RenderOutcome::Completed(Box::new(completed)));
        }
        if message.subject == RENDER_FAILED_SUBJECT.into()
            && let Ok(failed) = serde_json::from_slice::<RenderFailed>(&message.payload)
            && failed.render_id == command.render_id
        {
            return Ok(RenderOutcome::Failed(failed));
        }
    }
}
