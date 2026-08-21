//! Lease-backed publication from the transactional outbox.

use std::future::Future;
use std::time::Duration;

use async_nats::jetstream;
use sqlx::{PgPool, Row as _};

const EVENT_STREAM: &str = "ratatoskr_events";
const COMMAND_STREAM: &str = "ratatoskr_commands";

/// A bus publication failure.
#[derive(Debug, thiserror::Error)]
#[error("the message was not acknowledged by the bus")]
pub struct PublishError(#[source] Box<dyn std::error::Error + Send + Sync>);

impl PublishError {
    /// Wraps one transport failure without exposing it as a public response.
    #[must_use]
    pub fn new(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

/// A publisher that confirms durable broker responsibility.
pub trait Publisher: Send + Sync {
    /// Publishes one row and waits for a `JetStream` acknowledgement.
    fn publish(
        &self,
        subject: &str,
        payload: &[u8],
        message_id: &str,
    ) -> impl Future<Output = Result<(), PublishError>> + Send;
}

/// Connected `JetStream` publisher.
#[derive(Debug, Clone)]
pub struct NatsPublisher {
    client: async_nats::Client,
    context: jetstream::Context,
}

impl NatsPublisher {
    /// Connects to the configured NATS endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`PublishError`] when NATS is unavailable.
    pub async fn connect(url: &str) -> Result<Self, PublishError> {
        let client = async_nats::connect(url).await.map_err(PublishError::new)?;
        Ok(Self {
            context: jetstream::new(client.clone()),
            client,
        })
    }

    /// Connects with the nkey seed stored in a deployment file.
    ///
    /// # Errors
    ///
    /// Returns [`PublishError`] when the file, credentials, or NATS endpoint are unavailable.
    pub async fn connect_with_nkey(
        url: &str,
        seed_path: &std::path::Path,
    ) -> Result<Self, PublishError> {
        let seed = std::fs::read_to_string(seed_path).map_err(PublishError::new)?;
        let client = async_nats::ConnectOptions::with_nkey(seed.trim().to_owned())
            .connect(url)
            .await
            .map_err(PublishError::new)?;
        Ok(Self {
            context: jetstream::new(client.clone()),
            client,
        })
    }

    /// Reports the current connection state without network I/O.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(
            self.client.connection_state(),
            async_nats::connection::State::Connected
        )
    }

    /// Creates the bounded shared event stream when absent.
    ///
    /// # Errors
    ///
    /// Returns [`PublishError`] when the broker refuses stream management.
    pub async fn ensure_event_stream(&self) -> Result<(), PublishError> {
        self.ensure_stream(EVENT_STREAM, "evt.>", jetstream::stream::DiscardPolicy::Old)
            .await
    }

    /// Creates the bounded shared command stream when absent.
    ///
    /// # Errors
    ///
    /// Returns [`PublishError`] when the broker refuses stream management.
    pub async fn ensure_command_stream(&self) -> Result<(), PublishError> {
        self.ensure_stream(
            COMMAND_STREAM,
            "cmd.>",
            jetstream::stream::DiscardPolicy::New,
        )
        .await
    }

    /// Gives the durable consumer access to this connection's `JetStream` context.
    #[must_use]
    pub const fn context(&self) -> &jetstream::Context {
        &self.context
    }

    async fn ensure_stream(
        &self,
        name: &str,
        subject: &str,
        discard: jetstream::stream::DiscardPolicy,
    ) -> Result<(), PublishError> {
        self.context
            .get_or_create_stream(jetstream::stream::Config {
                name: name.to_owned(),
                subjects: vec![subject.to_owned()],
                retention: jetstream::stream::RetentionPolicy::Limits,
                storage: jetstream::stream::StorageType::File,
                discard,
                max_messages: 1_000_000,
                max_bytes: 1024 * 1024 * 1024,
                max_age: Duration::from_hours(168),
                duplicate_window: Duration::from_mins(2),
                num_replicas: 1,
                ..jetstream::stream::Config::default()
            })
            .await
            .map_err(PublishError::new)?;
        Ok(())
    }
}

impl Publisher for NatsPublisher {
    async fn publish(
        &self,
        subject: &str,
        payload: &[u8],
        message_id: &str,
    ) -> Result<(), PublishError> {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Msg-Id", message_id);
        let acknowledgement = self
            .context
            .publish_with_headers(subject.to_owned(), headers, payload.to_vec().into())
            .await
            .map_err(PublishError::new)?;
        acknowledgement.await.map_err(PublishError::new)?;
        Ok(())
    }
}

/// Outcome of one bounded outbox pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutboxReport {
    /// Rows leased.
    pub claimed: usize,
    /// Rows acknowledged by `JetStream`.
    pub published: usize,
    /// Rows backed off after failure.
    pub failed: usize,
    /// Rows that exhausted the retry ceiling.
    pub dead_lettered: usize,
}

/// Claims and attempts one finite batch.
///
/// # Errors
///
/// Returns `sqlx::Error` when lease or outcome persistence fails. Individual publication failures
/// are recorded on their rows and do not abort the remaining batch.
pub async fn run_outbox_once<P: Publisher>(
    pool: &PgPool,
    publisher: &P,
    claimed_by: &str,
    limit: i64,
) -> Result<OutboxReport, sqlx::Error> {
    let rows = sqlx::query(
        "with due as (
             select outbox_id from extractor.outbox_events
              where published_at is null and dead_lettered_at is null
                and next_attempt_at <= clock_timestamp()
                and (claimed_until is null or claimed_until <= clock_timestamp())
              order by next_attempt_at, enqueued_at
              limit $2 for update skip locked
         )
         update extractor.outbox_events o
            set claimed_until = clock_timestamp() + interval '30 seconds', claimed_by = $1
           from due where o.outbox_id = due.outbox_id
          returning o.outbox_id, o.message_id, o.subject, o.payload",
    )
    .bind(claimed_by)
    .bind(limit.clamp(1, 1_000))
    .fetch_all(pool)
    .await?;
    let mut report = OutboxReport {
        claimed: rows.len(),
        ..OutboxReport::default()
    };

    for row in rows {
        let outbox_id: uuid::Uuid = row.try_get("outbox_id")?;
        let message_id: uuid::Uuid = row.try_get("message_id")?;
        let subject: String = row.try_get("subject")?;
        let payload: serde_json::Value = row.try_get("payload")?;
        let body = match serde_json::to_vec(&payload) {
            Ok(body) => body,
            Err(error) => {
                report.failed += 1;
                if mark_failed(pool, outbox_id, &error.to_string()).await? {
                    report.dead_lettered += 1;
                }
                continue;
            }
        };
        match publisher
            .publish(&subject, &body, &message_id.to_string())
            .await
        {
            Ok(()) => {
                sqlx::query(
                    "update extractor.outbox_events
                        set published_at = clock_timestamp(), claimed_until = null,
                            claimed_by = null, last_error = null
                      where outbox_id = $1",
                )
                .bind(outbox_id)
                .execute(pool)
                .await?;
                report.published += 1;
                metrics::counter!("ratatoskr_extractor_outbox_publications_total", "outcome" => "published").increment(1);
            }
            Err(error) => {
                report.failed += 1;
                metrics::counter!("ratatoskr_extractor_outbox_publications_total", "outcome" => "failed").increment(1);
                if mark_failed(pool, outbox_id, &error.to_string()).await? {
                    report.dead_lettered += 1;
                    metrics::counter!("ratatoskr_extractor_outbox_publications_total", "outcome" => "dead_lettered").increment(1);
                }
            }
        }
    }
    Ok(report)
}

async fn mark_failed(
    pool: &PgPool,
    outbox_id: uuid::Uuid,
    error: &str,
) -> Result<bool, sqlx::Error> {
    let safe_error: String = error
        .chars()
        .filter(|character| !matches!(character, '\r' | '\n'))
        .take(512)
        .collect();
    sqlx::query_scalar(
        "update extractor.outbox_events
            set attempts = attempts + 1, last_error = $2,
                claimed_until = null, claimed_by = null,
                next_attempt_at = clock_timestamp()
                    + make_interval(secs => least(300.0, power(2.0, least(attempts, 20)))),
                dead_lettered_at = case when attempts + 1 >= 12 then clock_timestamp() end
          where outbox_id = $1
          returning dead_lettered_at is not null",
    )
    .bind(outbox_id)
    .bind(safe_error)
    .fetch_one(pool)
    .await
}
