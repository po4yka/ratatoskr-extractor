//! Durable capture-command consumption.

use async_nats::jetstream;
use futures_util::StreamExt as _;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::{ConsumeError, NatsPublisher, consume_capture};

/// Outcome of one joined command-consumer run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConsumerReport {
    /// New commands applied.
    pub applied: usize,
    /// Inbox duplicates absorbed.
    pub duplicates: usize,
    /// Poison commands acknowledged.
    pub malformed: usize,
    /// Transient deliveries left for redelivery.
    pub failed: usize,
}

/// Consumes capture commands until cancellation and acknowledges only durable outcomes.
///
/// # Errors
///
/// Returns [`crate::PublishError`] when stream or durable-consumer setup fails.
pub async fn run_command_consumer(
    publisher: &NatsPublisher,
    pool: &PgPool,
    durable_name: &str,
    cancellation: CancellationToken,
) -> Result<ConsumerReport, crate::PublishError> {
    publisher.ensure_command_stream().await?;
    let stream = publisher
        .context()
        .get_stream("ratatoskr_commands")
        .await
        .map_err(crate::PublishError::new)?;
    let consumer = stream
        .get_or_create_consumer(
            durable_name,
            jetstream::consumer::pull::Config {
                durable_name: Some(durable_name.to_owned()),
                filter_subject: "cmd.content.capture.requested.v1".to_owned(),
                ack_policy: jetstream::consumer::AckPolicy::Explicit,
                ack_wait: std::time::Duration::from_secs(30),
                max_deliver: 12,
                ..jetstream::consumer::pull::Config::default()
            },
        )
        .await
        .map_err(crate::PublishError::new)?;
    let mut messages = consumer
        .messages()
        .await
        .map_err(crate::PublishError::new)?;
    let mut report = ConsumerReport::default();
    loop {
        let message = tokio::select! {
            biased;
            () = cancellation.cancelled() => break,
            next = messages.next() => next,
        };
        let Some(message) = message else { break };
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                report.failed += 1;
                tracing::warn!(error = %error, "capture command delivery failed");
                continue;
            }
        };
        let subject = message.subject.to_string();
        match consume_capture(pool, &subject, &message.payload).await {
            Ok(crate::Reception::Applied) => {
                report.applied += 1;
                count("applied");
            }
            Ok(crate::Reception::Duplicate) => {
                report.duplicates += 1;
                count("duplicate");
            }
            Err(error) if should_redeliver(&error) => {
                report.failed += 1;
                count("failed");
                tracing::warn!(error = %error, "capture command persistence failed");
                continue;
            }
            Err(error) => {
                report.malformed += 1;
                count("rejected");
                tracing::warn!(error = %error, "capture command was rejected");
            }
        }
        if let Err(error) = message.ack().await {
            report.failed += 1;
            tracing::warn!(error = %error, "capture command acknowledgement failed");
        }
    }
    Ok(report)
}

fn count(outcome: &'static str) {
    metrics::counter!("ratatoskr_extractor_commands_total", "outcome" => outcome).increment(1);
}

const fn should_redeliver(error: &ConsumeError) -> bool {
    matches!(error, ConsumeError::Database(_))
}
