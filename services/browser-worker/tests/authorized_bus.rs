//! The browser worker under its narrow NATS identity against an authorization-enabled broker
//! (XR-021 CONTRACTS.md S03 and S04).
//!
//! An administrator provisions the topology the way Edge does; the worker identity then consumes,
//! publishes and marks completions with exactly the permissions of
//! `deploy/nats/identity-browser-worker.conf`. A denied publish is visible to the server log only,
//! so every refusal is observed through a missing acknowledgement or an error.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::consumer::{AckPolicy, pull};
use async_nats::jetstream::{self, Context};
use browser_worker::{
    RenderExecutor, RenderOutcome, WorkerError, WorkerSettings, connect, run_render_consumer,
};
use extractor_test_support::TemporaryBlobRoot;
use extractor_test_support::broker::{AuthorizedBroker, TestIdentity};
use futures_util::StreamExt as _;
use render_job::{
    NetworkEvidence, RENDER_COMPLETED_SUBJECT, RENDER_REQUESTED_SUBJECT, RenderBudgets,
    RenderCommand, RenderCompleted,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

// These names and settings must equal `platform_eventing::stream`; the workspace check pins the
// permission stanza that grants them.
const COMMANDS: &str = "ratatoskr_commands";
const EVENTS: &str = "ratatoskr_events";
const DURABLE: &str = "ratatoskr_browser_worker";
const BUCKET: &str = "browser_worker_completions";

#[derive(Default)]
struct StubExecutor {
    invocations: Mutex<Vec<RenderCommand>>,
}

struct SharedStub(Arc<StubExecutor>);

impl RenderExecutor for SharedStub {
    async fn render(&self, command: &RenderCommand) -> Result<RenderOutcome, WorkerError> {
        self.0.invocations.lock().await.push(command.clone());
        Ok(RenderOutcome {
            dom: b"<html><body>rendered</body></html>".to_vec(),
            final_url: command.url.clone(),
            evidence: NetworkEvidence {
                hops: Vec::new(),
                blocked_requests: 0,
            },
        })
    }
}

fn command() -> RenderCommand {
    RenderCommand {
        render_id: uuid::Uuid::now_v7(),
        operation_id: uuid::Uuid::now_v7(),
        correlation_id: "operation:authorized-bus".to_owned(),
        tenant_user_id: uuid::Uuid::now_v7(),
        url: "https://example.test/app".to_owned(),
        budgets: RenderBudgets {
            navigation_timeout_ms: 5_000,
            total_timeout_ms: 10_000,
            max_dom_bytes: 65_536,
        },
    }
}

struct Harness {
    broker: AuthorizedBroker,
    admin_client: async_nats::Client,
    admin: Context,
    settings: WorkerSettings,
    _blobs: TemporaryBlobRoot,
}

impl Harness {
    /// Starts the broker with the shipped fragment and, when `topology` is set, provisions both
    /// streams, the fixed durable and the completions bucket as an administrator.
    async fn start(topology: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let identity = TestIdentity::generate()?;
        let fragment = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/nats/identity-browser-worker.conf"
        ))?;
        let broker = AuthorizedBroker::start(&identity.substitute_into(&fragment)?).await?;
        let seed = identity.write_seed(broker.directory(), "browser-worker")?;
        let admin_client = async_nats::ConnectOptions::with_user_and_password(
            AuthorizedBroker::ADMIN_USER.to_owned(),
            broker.admin_password().to_owned(),
        )
        .connect(broker.url())
        .await?;
        let admin = jetstream::new(admin_client.clone());
        if topology {
            admin
                .create_stream(jetstream::stream::Config {
                    name: COMMANDS.to_owned(),
                    subjects: vec!["cmd.>".to_owned()],
                    ..jetstream::stream::Config::default()
                })
                .await?;
            admin
                .create_stream(jetstream::stream::Config {
                    name: EVENTS.to_owned(),
                    subjects: vec!["evt.>".to_owned()],
                    ..jetstream::stream::Config::default()
                })
                .await?;
            admin
                .create_consumer_on_stream(
                    pull::Config {
                        durable_name: Some(DURABLE.to_owned()),
                        filter_subject: RENDER_REQUESTED_SUBJECT.to_owned(),
                        ack_policy: AckPolicy::Explicit,
                        ack_wait: Duration::from_mins(5),
                        max_deliver: 12,
                        ..pull::Config::default()
                    },
                    COMMANDS,
                )
                .await?;
            admin
                .create_key_value(jetstream::kv::Config {
                    bucket: BUCKET.to_owned(),
                    max_age: Duration::from_hours(24),
                    ..jetstream::kv::Config::default()
                })
                .await?;
        }
        let blobs = TemporaryBlobRoot::create().await?;
        let settings = WorkerSettings {
            nats_url: broker.url().to_owned(),
            chrome_bin: None,
            blobs_root: blobs.path().to_path_buf(),
            durable_name: DURABLE.to_owned(),
            completions_bucket: BUCKET.to_owned(),
            max_jobs_per_process: u32::MAX,
            nkey_seed_path: Some(seed),
            provision_topology: false,
        };
        Ok(Self {
            broker,
            admin_client,
            admin,
            settings,
            _blobs: blobs,
        })
    }
}

#[tokio::test]
async fn the_worker_identity_renders_without_creating_topology()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::start(true).await?;
    let worker_client = connect(&harness.settings).await?;
    let render = command();
    let mut completions = harness
        .admin_client
        .subscribe(RENDER_COMPLETED_SUBJECT)
        .await?;
    harness.admin_client.flush().await?;

    let executor = Arc::new(StubExecutor::default());
    let cancellation = CancellationToken::new();
    let worker = tokio::spawn(run_render_consumer(
        jetstream::new(worker_client),
        harness.settings.clone(),
        SharedStub(executor.clone()),
        cancellation.clone(),
    ));
    harness
        .admin
        .publish(
            RENDER_REQUESTED_SUBJECT,
            serde_json::to_vec(&render)?.into(),
        )
        .await?
        .await?;

    let completed: RenderCompleted = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let message = completions.next().await.ok_or("the subscription ended")?;
            let decoded: RenderCompleted = serde_json::from_slice(&message.payload)?;
            if decoded.render_id == render.render_id {
                return Ok::<_, Box<dyn std::error::Error>>(decoded);
            }
        }
    })
    .await??;
    assert_eq!(completed.final_url, render.url);

    // The marker was written through the narrow `$KV` grant and the delivery was acknowledged
    // through the narrow `$JS.ACK` grant.
    let bucket = harness.admin.get_key_value(BUCKET).await?;
    let mut marked = false;
    for _ in 0..100 {
        if bucket.get(render.render_id.to_string()).await?.is_some() {
            marked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(marked, "the completion marker must be written");
    let mut durable = harness
        .admin
        .get_consumer_from_stream::<pull::Config, _, _>(DURABLE, COMMANDS)
        .await?;
    let mut acknowledged = false;
    for _ in 0..100 {
        let info = durable.info().await?;
        if info.num_ack_pending == 0 && info.ack_floor.stream_sequence == 1 {
            acknowledged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(acknowledged, "the delivery must be acknowledged");
    assert_eq!(executor.invocations.lock().await.len(), 1);

    cancellation.cancel();
    worker.await??;
    drop(harness.broker);
    Ok(())
}

#[tokio::test]
async fn the_broker_refuses_everything_outside_the_stanza() -> Result<(), Box<dyn std::error::Error>>
{
    let harness = Harness::start(true).await?;
    let worker = jetstream::new(connect(&harness.settings).await?);

    // Another producer's fact is refused: a denied publish never receives an acknowledgement.
    let foreign = async {
        worker
            .publish("evt.platform.operation.reported.v1", "{}".into())
            .await?
            .await
    }
    .await;
    assert!(
        foreign.is_err(),
        "evt.platform.operation.reported.v1 must be refused"
    );

    // Topology creation is refused: no stream, no consumer, no bucket.
    assert!(
        worker
            .create_stream(jetstream::stream::Config {
                name: "worker_private".to_owned(),
                subjects: vec!["evt.worker.private.>".to_owned()],
                ..jetstream::stream::Config::default()
            })
            .await
            .is_err()
    );
    assert!(
        worker
            .create_consumer_on_stream(
                pull::Config {
                    durable_name: Some("worker_private".to_owned()),
                    ..pull::Config::default()
                },
                COMMANDS,
            )
            .await
            .is_err()
    );
    assert!(
        worker
            .create_key_value(jetstream::kv::Config {
                bucket: "worker_private".to_owned(),
                ..jetstream::kv::Config::default()
            })
            .await
            .is_err()
    );
    assert!(worker.get_stream(COMMANDS).await.is_err());
    Ok(())
}

#[tokio::test]
async fn missing_topology_stops_the_worker_and_is_never_created()
-> Result<(), Box<dyn std::error::Error>> {
    // Nothing is provisioned: Edge has not started yet.
    let harness = Harness::start(false).await?;
    let worker = jetstream::new(connect(&harness.settings).await?);

    let outcome = Box::pin(run_render_consumer(
        worker,
        harness.settings.clone(),
        SharedStub(Arc::new(StubExecutor::default())),
        CancellationToken::new(),
    ))
    .await;

    assert!(outcome.is_err(), "missing topology must stop the worker");
    assert!(harness.admin.get_stream(COMMANDS).await.is_err());
    assert!(harness.admin.get_key_value(BUCKET).await.is_err());
    Ok(())
}
