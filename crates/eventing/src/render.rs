//! Extractor-side render requests to the browser worker.

use async_nats::jetstream;
use futures_util::StreamExt as _;

use crate::{ConsumeError, topology};
use render_job::{
    RENDER_COMPLETED_SUBJECT, RENDER_FAILED_SUBJECT, RENDER_REQUESTED_SUBJECT, RenderCommand,
    RenderCompleted, RenderFailed,
};

const RENDER_FETCH_BATCH: usize = 32;
const RENDER_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// A `JetStream` handle together with whether this process may create the topology it uses.
#[derive(Debug, Clone)]
pub struct RenderBus {
    context: jetstream::Context,
    provision_topology: bool,
}

impl RenderBus {
    /// Wraps `context`; `provision_topology` is true only for an unauthenticated development broker.
    #[must_use]
    pub const fn new(context: jetstream::Context, provision_topology: bool) -> Self {
        Self {
            context,
            provision_topology,
        }
    }
}

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
    bus: &RenderBus,
    command: &RenderCommand,
) -> Result<RenderOutcome, RenderRequestError> {
    let context = &bus.context;
    let payload = serde_json::to_vec(command).map_err(infrastructure)?;
    let acknowledgement = context
        .publish(RENDER_REQUESTED_SUBJECT, payload.into())
        .await
        .map_err(infrastructure)?;
    acknowledgement.await.map_err(infrastructure)?;

    let consumer = if bus.provision_topology {
        context
            .get_stream(crate::EVENTS_STREAM)
            .await
            .map_err(infrastructure)?
            .get_or_create_consumer(
                topology::RENDER_AWAITS_DURABLE,
                topology::render_awaits_config(),
            )
            .await
            .map_err(infrastructure)?
    } else {
        topology::render_awaits_consumer(context)
            .await
            .map_err(infrastructure)?
    };

    let total = std::time::Duration::from_millis(command.budgets.total_timeout_ms);
    let deadline = tokio::time::Instant::now() + total;
    loop {
        // One non-waiting pull per pass: the durable is shared by every request, so no pull
        // request may stay pending on the server after this call returns.
        let mut batch = consumer
            .fetch()
            .max_messages(RENDER_FETCH_BATCH)
            .messages()
            .await
            .map_err(infrastructure)?;
        while let Some(delivery) = batch.next().await {
            let Ok(message) = delivery else { break };
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
        if tokio::time::Instant::now() >= deadline {
            return Err(RenderRequestError::Timeout);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::sleep(RENDER_POLL_INTERVAL.min(remaining)).await;
    }
}
