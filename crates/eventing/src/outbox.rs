//! Lease-backed publication from the transactional outbox.

use std::future::Future;
use std::time::Duration;

use async_nats::jetstream;
use sqlx::{PgPool, Row as _};

use crate::topology::INBOX_PREFIX;

const EVENT_STREAM: &str = "ratatoskr_events";
const COMMAND_STREAM: &str = "ratatoskr_commands";

/// Headroom kept under the server's `max_payload` for the subject and headers.
const PAYLOAD_HEADROOM_BYTES: usize = 1024;

/// A bus publication failure, classed so the outbox knows whether a retry can ever help.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PublishError {
    /// The broker was unreachable, refused the request, or did not acknowledge it in time. A
    /// publish without an acknowledgement is indistinguishable from a permission denial, so the
    /// message tells the operator where the difference shows.
    #[error(
        "the message was not acknowledged by the bus; check the NATS server log for a Publish Violation"
    )]
    Transient(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The serialized message exceeds what the connected server accepts, so no retry can succeed.
    #[error("the message is larger than the bus accepts")]
    TooLarge,
}

impl PublishError {
    /// Wraps one transport failure without exposing it as a public response.
    #[must_use]
    pub fn new(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Transient(Box::new(error))
    }

    /// The class of a message that is larger than the connected server's limit.
    #[must_use]
    pub const fn too_large() -> Self {
        Self::TooLarge
    }

    /// `true` when retrying the same message can never succeed.
    #[must_use]
    pub const fn is_permanent(&self) -> bool {
        matches!(self, Self::TooLarge)
    }

    /// A fixed, content-free class for the row's `last_error`, logs and metric labels.
    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self {
            Self::Transient(_) => "publish_failed",
            Self::TooLarge => PAYLOAD_TOO_LARGE,
        }
    }
}

const PAYLOAD_TOO_LARGE: &str = "payload_too_large";
const PAYLOAD_UNSERIALIZABLE: &str = "payload_unserializable";

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
        Self::connect_with_options(url, async_nats::ConnectOptions::new()).await
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
        Self::connect_with_options(
            url,
            async_nats::ConnectOptions::with_nkey(seed.trim().to_owned()),
        )
        .await
    }

    /// Connects with caller-supplied authentication, for example an administrator that provisions
    /// topology the way Edge does. The connection always replies on the extractor's own inbox
    /// prefix, so no caller can build a connection the narrow stanza would starve.
    ///
    /// # Errors
    ///
    /// Returns [`PublishError`] when NATS is unavailable or refuses the credentials.
    pub async fn connect_with_options(
        url: &str,
        options: async_nats::ConnectOptions,
    ) -> Result<Self, PublishError> {
        let client = options
            .custom_inbox_prefix(INBOX_PREFIX)
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
        ensure_event_stream_on(&self.context).await
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
        // The server refuses a larger message with no useful reply; classing it here keeps the
        // outbox from retrying a row that can never be delivered.
        let limit = self
            .client
            .server_info()
            .max_payload
            .saturating_sub(PAYLOAD_HEADROOM_BYTES);
        if payload.len() > limit {
            return Err(PublishError::too_large());
        }
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
    /// Rows backed off after a transient failure; they are retried without limit.
    pub failed: usize,
    /// Rows marked undeliverable after a permanent failure, such as a message larger than the
    /// server accepts. No transient failure ever counts here.
    pub dead_lettered: usize,
}

/// Failed attempts of the oldest unpublished row after which a refusing publisher is unhealthy.
const UNHEALTHY_ATTEMPTS: i32 = 3;
/// Age in seconds of the oldest unpublished row after which a refusing publisher is unhealthy.
const UNHEALTHY_AGE_SECONDS: i32 = 300;

/// Whether the relay can currently publish what it holds (XR-021 CONTRACTS.md R2-03).
///
/// The verdict is unhealthy when a pass attempted a publish, at least one publish failed, and the
/// oldest unpublished publishable row has failed at least three times or is older than 300
/// seconds. It turns healthy again after a pass that publishes every due row, or when nothing is
/// left unpublished. A row marked undeliverable is outside the retry set and never counts. The
/// verdict only informs readiness and a counter: the relay keeps retrying whatever it says.
#[derive(Debug, Clone, Copy)]
pub struct OutboxHealth {
    healthy: bool,
}

impl OutboxHealth {
    /// A relay that has not failed yet.
    #[must_use]
    pub const fn new() -> Self {
        Self { healthy: true }
    }

    /// The verdict after the latest observed pass.
    #[must_use]
    pub const fn is_healthy(&self) -> bool {
        self.healthy
    }

    /// Folds one finished pass into the verdict and returns it.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` when the oldest-row query fails.
    pub async fn observe(
        &mut self,
        pool: &PgPool,
        report: &OutboxReport,
    ) -> Result<bool, sqlx::Error> {
        let head = oldest_unpublished(pool).await?;
        let healthy = if report.failed > 0 {
            !head.as_ref().is_some_and(|head| head.stuck)
        } else if report.claimed == 0 {
            // Nothing was due. A row waiting out its backoff says nothing about the publisher,
            // so only an empty queue is evidence of health.
            head.is_none() || self.healthy
        } else {
            true
        };
        if self.healthy && !healthy {
            let class = head
                .and_then(|head| head.last_error)
                .unwrap_or_else(|| "publish_failed".to_owned());
            tracing::error!(%class, "outbox publisher is unhealthy; readiness is withdrawn");
            metrics::counter!("ratatoskr_extractor_outbox_publisher_unhealthy_total", "class" => class)
                .increment(1);
        }
        self.healthy = healthy;
        Ok(healthy)
    }
}

/// The oldest row still in the retry set and whether it is stuck.
struct OldestUnpublished {
    stuck: bool,
    last_error: Option<String>,
}

async fn oldest_unpublished(pool: &PgPool) -> Result<Option<OldestUnpublished>, sqlx::Error> {
    let row = sqlx::query(
        "select attempts >= $1
                or enqueued_at < clock_timestamp() - make_interval(secs => $2) as stuck,
                last_error
           from extractor.outbox_events
          where published_at is null and dead_lettered_at is null
          order by enqueued_at, outbox_id
          limit 1",
    )
    .bind(UNHEALTHY_ATTEMPTS)
    .bind(f64::from(UNHEALTHY_AGE_SECONDS))
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(OldestUnpublished {
            stuck: row.try_get("stuck")?,
            last_error: row.try_get("last_error")?,
        })
    })
    .transpose()
}

impl Default for OutboxHealth {
    fn default() -> Self {
        Self::new()
    }
}

/// Claims and attempts one finite batch.
///
/// A transient failure backs the row off and never gives up on it; only a permanent failure marks
/// a row undeliverable, and the pass continues with the next row either way.
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
        let Ok(body) = serde_json::to_vec(&payload) else {
            mark_undeliverable(pool, outbox_id, PAYLOAD_UNSERIALIZABLE).await?;
            report.dead_lettered += 1;
            continue;
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
            Err(error) if error.is_permanent() => {
                mark_undeliverable(pool, outbox_id, error.class()).await?;
                report.dead_lettered += 1;
            }
            Err(error) => {
                tracing::warn!(class = error.class(), %error, "outbox publish failed; retrying with backoff");
                mark_transient(pool, outbox_id, error.class()).await?;
                report.failed += 1;
                metrics::counter!("ratatoskr_extractor_outbox_publications_total", "outcome" => "failed").increment(1);
            }
        }
    }
    Ok(report)
}

/// Records one failed attempt and the capped backoff. The row is never given up on.
async fn mark_transient(
    pool: &PgPool,
    outbox_id: uuid::Uuid,
    class: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update extractor.outbox_events
            set attempts = attempts + 1, last_error = $2,
                claimed_until = null, claimed_by = null,
                next_attempt_at = clock_timestamp()
                    + make_interval(secs => least(300.0, power(2.0, least(attempts, 20))))
          where outbox_id = $1",
    )
    .bind(outbox_id)
    .bind(class)
    .execute(pool)
    .await?;
    Ok(())
}

/// Takes the row out of the retry set because no retry can succeed.
async fn mark_undeliverable(
    pool: &PgPool,
    outbox_id: uuid::Uuid,
    class: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update extractor.outbox_events
            set attempts = attempts + 1, last_error = $2,
                claimed_until = null, claimed_by = null,
                dead_lettered_at = clock_timestamp()
          where outbox_id = $1",
    )
    .bind(outbox_id)
    .bind(class)
    .execute(pool)
    .await?;
    tracing::error!(class, "outbox row is undeliverable");
    metrics::counter!("ratatoskr_extractor_outbox_publications_total", "outcome" => "undeliverable").increment(1);
    metrics::counter!("ratatoskr_extractor_outbox_undeliverable_total", "class" => class.to_owned())
        .increment(1);
    Ok(())
}

/// Creates or loads the shared fleet event stream for a fresh deployment or test broker.
///
/// # Errors
///
/// Returns [`PublishError`] when `JetStream` setup fails.
pub async fn ensure_event_stream_on(context: &jetstream::Context) -> Result<(), PublishError> {
    context
        .get_or_create_stream(jetstream::stream::Config {
            name: EVENT_STREAM.to_owned(),
            subjects: vec!["evt.>".to_owned()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            storage: jetstream::stream::StorageType::File,
            discard: jetstream::stream::DiscardPolicy::Old,
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
