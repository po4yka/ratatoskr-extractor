//! Durable capture-command consumption.

use futures_util::StreamExt as _;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::{ConsumeError, NatsPublisher, consume_capture, topology};

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
/// With `provision_topology` false the capture durable must already exist as Edge specified it; it
/// is fetched and verified, never created. With it true (an unauthenticated development broker
/// only) the stream and durable are created when absent.
///
/// # Errors
///
/// Returns [`crate::PublishError`] when the durable is absent or different (verified mode) or when
/// stream or durable setup fails (provisioning mode).
pub async fn run_command_consumer(
    publisher: &NatsPublisher,
    pool: &PgPool,
    durable_name: &str,
    provision_topology: bool,
    cancellation: CancellationToken,
) -> Result<ConsumerReport, crate::PublishError> {
    let consumer = if provision_topology {
        publisher.ensure_command_stream().await?;
        publisher
            .context()
            .get_stream(crate::COMMAND_STREAM)
            .await
            .map_err(crate::PublishError::new)?
            .get_or_create_consumer(durable_name, topology::capture_config(durable_name))
            .await
            .map_err(crate::PublishError::new)?
    } else {
        topology::capture_consumer(publisher.context(), durable_name)
            .await
            .map_err(crate::PublishError::new)?
    };
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
