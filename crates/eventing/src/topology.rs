//! The Edge-provisioned durables the extractor depends on (XR-021 CONTRACTS.md S02 rule 6, S04).
//!
//! A production process fetches these by name and verifies them against their specification; it
//! never creates a stream, a consumer or a bucket. A development process against an
//! unauthenticated broker may provision them itself.

use std::time::Duration;

use async_nats::jetstream::Context;
use async_nats::jetstream::consumer::{AckPolicy, Consumer, DeliverPolicy, pull};

use crate::{COMMAND_STREAM, EVENTS_STREAM, NatsPublisher};

/// The only capture durable a production process may consume.
pub const CAPTURE_DURABLE: &str = "ratatoskr_extractor_capture";
/// Subject filter of the capture durable.
pub(crate) const CAPTURE_FILTER: &str = "cmd.content.capture.requested.v1";
const CAPTURE_ACK_WAIT: Duration = Duration::from_secs(30);
const CAPTURE_MAX_DELIVER: i64 = 12;

/// The durable the extractor reads render results from.
pub(crate) const RENDER_AWAITS_DURABLE: &str = "ratatoskr_extractor_render_awaits";
const RENDER_AWAITS_FILTER: &str = "evt.content.render.>";
const RENDER_AWAITS_ACK_WAIT: Duration = Duration::from_secs(30);

/// A durable the extractor depends on is absent, unreachable or differs from its specification.
#[derive(Debug, thiserror::Error)]
#[error(
    "the durable `{durable}` is missing, unreachable, not permitted or differs from its \
     specification; start ratatoskr-edge first so it provisions the topology"
)]
pub struct TopologyError {
    durable: String,
}

impl TopologyError {
    fn new(durable: &str) -> Self {
        Self {
            durable: durable.to_owned(),
        }
    }
}

/// Configuration used when a development process creates the capture durable itself.
pub(crate) fn capture_config(durable_name: &str) -> pull::Config {
    pull::Config {
        durable_name: Some(durable_name.to_owned()),
        filter_subject: CAPTURE_FILTER.to_owned(),
        ack_policy: AckPolicy::Explicit,
        ack_wait: CAPTURE_ACK_WAIT,
        max_deliver: CAPTURE_MAX_DELIVER,
        ..pull::Config::default()
    }
}

/// Configuration used when a development process creates the render-await consumer itself.
pub(crate) fn render_awaits_config() -> pull::Config {
    pull::Config {
        durable_name: None,
        filter_subject: RENDER_AWAITS_FILTER.to_owned(),
        ack_policy: AckPolicy::None,
        inactive_threshold: Duration::from_mins(2),
        ..pull::Config::default()
    }
}

/// Fetches the capture durable and verifies it against its specification.
pub(crate) async fn capture_consumer(
    context: &Context,
    durable_name: &str,
) -> Result<Consumer<pull::Config>, TopologyError> {
    let consumer = context
        .get_consumer_from_stream::<pull::Config, _, _>(durable_name, COMMAND_STREAM)
        .await
        .map_err(|_| TopologyError::new(durable_name))?;
    let config = &consumer.cached_info().config;
    let matches = durable_name == CAPTURE_DURABLE
        && config.durable_name.as_deref() == Some(durable_name)
        && config.filter_subject == CAPTURE_FILTER
        && config.ack_policy == AckPolicy::Explicit
        && config.ack_wait == CAPTURE_ACK_WAIT
        && config.max_deliver == CAPTURE_MAX_DELIVER;
    if matches {
        Ok(consumer)
    } else {
        Err(TopologyError::new(durable_name))
    }
}

/// Fetches the render-await durable and verifies it against its specification.
pub(crate) async fn render_awaits_consumer(
    context: &Context,
) -> Result<Consumer<pull::Config>, TopologyError> {
    let consumer = context
        .get_consumer_from_stream::<pull::Config, _, _>(RENDER_AWAITS_DURABLE, EVENTS_STREAM)
        .await
        .map_err(|_| TopologyError::new(RENDER_AWAITS_DURABLE))?;
    let config = &consumer.cached_info().config;
    let matches = config.durable_name.as_deref() == Some(RENDER_AWAITS_DURABLE)
        && config.filter_subject == RENDER_AWAITS_FILTER
        && config.ack_policy == AckPolicy::None
        && config.deliver_policy == DeliverPolicy::New
        && config.ack_wait == RENDER_AWAITS_ACK_WAIT;
    if matches {
        Ok(consumer)
    } else {
        Err(TopologyError::new(RENDER_AWAITS_DURABLE))
    }
}

/// Checks that Edge provisioned every durable this process uses, without creating anything.
///
/// # Errors
///
/// Returns [`TopologyError`] naming the first durable that is absent, unreachable or different.
pub async fn verify_bus_topology(
    publisher: &NatsPublisher,
    durable_name: &str,
) -> Result<(), TopologyError> {
    capture_consumer(publisher.context(), durable_name).await?;
    render_awaits_consumer(publisher.context()).await?;
    Ok(())
}
