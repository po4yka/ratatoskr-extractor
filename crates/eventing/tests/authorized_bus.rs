//! The extractor under its narrow NATS identity against an authorization-enabled broker
//! (XR-021 CONTRACTS.md S03 and S04).
//!
//! An administrator provisions the topology the way Edge does; the extractor identity then
//! consumes, publishes and awaits renders with exactly the permissions of
//! `deploy/nats/identity.conf`. A denied publish is visible to the server log only, so every
//! refusal is observed through a missing acknowledgement or an error.

use std::path::PathBuf;
use std::time::Duration;

use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, pull};
use async_nats::jetstream::{self, Context};
use extractor_eventing::{
    NatsPublisher, Publisher, RenderBus, RenderOutcome, request_render, run_command_consumer,
    run_outbox_once, verify_bus_topology,
};
use extractor_persistence::test_support::TestDatabase;
use extractor_test_support::broker::{AuthorizedBroker, TestIdentity};
use extractor_test_support::capture::CaptureCommandJson;
use futures_util::StreamExt as _;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, MediaType,
};
use render_job::{
    NetworkEvidence, RENDER_COMPLETED_SUBJECT, RENDER_REQUESTED_SUBJECT, RenderBudgets,
    RenderCommand, RenderCompleted,
};
use tokio_util::sync::CancellationToken;

// These names and settings must equal `platform_eventing::stream`; the workspace check pins the
// permission stanza that grants them.
const COMMANDS: &str = "ratatoskr_commands";
const EVENTS: &str = "ratatoskr_events";
const CAPTURE_DURABLE: &str = "ratatoskr_extractor_capture";
const CAPTURE_SUBJECT: &str = "cmd.content.capture.requested.v1";
const RENDER_AWAITS_DURABLE: &str = "ratatoskr_extractor_render_awaits";
const FOREIGN_DURABLE: &str = "ratatoskr_x_browser_capture";

struct Harness {
    broker: AuthorizedBroker,
    admin: Context,
    seed: PathBuf,
}

impl Harness {
    /// Starts the broker with the shipped fragment and, when `durables` is set, provisions both
    /// streams and the fixed durables as an administrator.
    async fn start(durables: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let identity = TestIdentity::generate()?;
        let fragment = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/nats/identity.conf"
        ))?;
        let broker = AuthorizedBroker::start(&identity.substitute_into(&fragment)?).await?;
        let seed = identity.write_seed(broker.directory(), "extractor")?;
        let administrator = NatsPublisher::connect_with_options(
            broker.url(),
            async_nats::ConnectOptions::with_user_and_password(
                AuthorizedBroker::ADMIN_USER.to_owned(),
                broker.admin_password().to_owned(),
            ),
        )
        .await?;
        administrator.ensure_command_stream().await?;
        administrator.ensure_event_stream().await?;
        let admin = administrator.context().clone();
        if durables {
            create_capture_durable(&admin, Duration::from_secs(30)).await?;
            admin
                .create_consumer_on_stream(
                    pull::Config {
                        durable_name: Some(RENDER_AWAITS_DURABLE.to_owned()),
                        filter_subject: "evt.content.render.>".to_owned(),
                        ack_policy: AckPolicy::None,
                        deliver_policy: DeliverPolicy::New,
                        ack_wait: Duration::from_secs(30),
                        ..pull::Config::default()
                    },
                    EVENTS,
                )
                .await?;
            admin
                .create_consumer_on_stream(
                    pull::Config {
                        durable_name: Some(FOREIGN_DURABLE.to_owned()),
                        filter_subject: "cmd.x.capture.requested.v1".to_owned(),
                        ack_policy: AckPolicy::Explicit,
                        ..pull::Config::default()
                    },
                    COMMANDS,
                )
                .await?;
        }
        Ok(Self {
            broker,
            admin,
            seed,
        })
    }

    async fn extractor(&self) -> Result<NatsPublisher, Box<dyn std::error::Error>> {
        Ok(NatsPublisher::connect_with_nkey(self.broker.url(), &self.seed).await?)
    }
}

async fn create_capture_durable(
    admin: &Context,
    ack_wait: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    admin
        .create_consumer_on_stream(
            pull::Config {
                durable_name: Some(CAPTURE_DURABLE.to_owned()),
                filter_subject: CAPTURE_SUBJECT.to_owned(),
                ack_policy: AckPolicy::Explicit,
                ack_wait,
                max_deliver: 12,
                ..pull::Config::default()
            },
            COMMANDS,
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn the_extractor_identity_consumes_and_publishes_without_creating_topology()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::start(true).await?;
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let capture = CaptureCommandJson::url("https://example.test/article");
    harness
        .admin
        .publish(CAPTURE_SUBJECT, capture.to_bytes().into())
        .await?
        .await?;

    let publisher = harness.extractor().await?;
    let cancellation = CancellationToken::new();
    let consumer = tokio::spawn({
        let publisher = publisher.clone();
        let pool = pool.clone();
        let cancellation = cancellation.clone();
        async move {
            run_command_consumer(&publisher, &pool, CAPTURE_DURABLE, false, cancellation).await
        }
    });
    wait_for(|| async {
        let applied: i64 = sqlx::query_scalar("select count(*) from extractor.inbox_events")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        applied == 1
    })
    .await?;
    cancellation.cancel();
    let report = consumer.await??;
    assert_eq!(report.applied, 1);

    // The queued report is the outbox row the capture itself produced; the document fact is the
    // other subject this identity may publish.
    let command_id: uuid::Uuid =
        sqlx::query_scalar("select command_id from extractor.inbox_events")
            .fetch_one(pool)
            .await?;
    sqlx::query(
        "insert into extractor.outbox_events
             (outbox_id, message_id, causation_command_id, operation_id, subject, payload,
              enqueued_at, next_attempt_at)
         values ($1, $2, $3, $4, 'evt.content.document.extracted.v1', '{\"probe\": true}'::jsonb,
                 now(), now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(command_id)
    .bind(capture.operation_id)
    .execute(pool)
    .await?;
    let outbox = run_outbox_once(pool, &publisher, "authorized-bus-test", 10).await?;
    assert_eq!((outbox.claimed, outbox.published, outbox.failed), (2, 2, 0));

    let mut events = harness.admin.get_stream(EVENTS).await?;
    assert_eq!(events.info().await?.state.messages, 2);
    // Acknowledgements travelled through the narrow `$JS.ACK` grant: nothing stays pending.
    let mut durable = harness
        .admin
        .get_consumer_from_stream::<pull::Config, _, _>(CAPTURE_DURABLE, COMMANDS)
        .await?;
    let info = durable.info().await?;
    assert_eq!(
        (info.num_ack_pending, info.ack_floor.stream_sequence),
        (0, 1)
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_render_request_is_awaited_through_the_fixed_durable()
-> Result<(), Box<dyn std::error::Error>> {
    let harness = Harness::start(true).await?;
    let publisher = harness.extractor().await?;
    let render_id = uuid::Uuid::now_v7();

    // The stub plays the browser worker: it answers the one request with a completion.
    let admin_client = async_nats::ConnectOptions::with_user_and_password(
        AuthorizedBroker::ADMIN_USER.to_owned(),
        harness.broker.admin_password().to_owned(),
    )
    .connect(harness.broker.url())
    .await?;
    let mut requests = admin_client.subscribe(RENDER_REQUESTED_SUBJECT).await?;
    // The subscription is queued locally; flush so the broker has registered it before the
    // extractor connection publishes the request.
    admin_client.flush().await?;
    let admin = harness.admin.clone();
    let stub = tokio::spawn(async move {
        let Some(message) = requests.next().await else {
            return Err("the render request never arrived".to_owned());
        };
        let command: RenderCommand =
            serde_json::from_slice(&message.payload).map_err(|error| error.to_string())?;
        let completed = RenderCompleted {
            render_id: command.render_id,
            final_url: command.url,
            dom: BlobRef {
                owner_service: BlobOwner::parse("ratatoskr-browser-worker")
                    .map_err(|error| error.to_string())?,
                digest: ContentDigest {
                    algorithm: DigestAlgorithm::Sha256,
                    hex: DigestHex::parse(&"a".repeat(64)).map_err(|error| error.to_string())?,
                },
                media_type: MediaType::parse("text/html").map_err(|error| error.to_string())?,
                length_bytes: 10,
            },
            evidence: NetworkEvidence {
                hops: Vec::new(),
                blocked_requests: 0,
            },
        };
        admin
            .publish(
                RENDER_COMPLETED_SUBJECT,
                serde_json::to_vec(&completed)
                    .map_err(|error| error.to_string())?
                    .into(),
            )
            .await
            .map_err(|error| error.to_string())?
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    });

    let command = RenderCommand {
        render_id,
        operation_id: uuid::Uuid::now_v7(),
        correlation_id: "operation:authorized-bus".to_owned(),
        tenant_user_id: uuid::Uuid::now_v7(),
        url: "https://example.test/app".to_owned(),
        budgets: RenderBudgets {
            navigation_timeout_ms: 2_000,
            total_timeout_ms: 15_000,
            max_dom_bytes: 4_096,
        },
    };
    let bus = RenderBus::new(publisher.context().clone(), false);
    let outcome = request_render(&bus, &command).await?;
    stub.await??;

    let RenderOutcome::Completed(completed) = outcome else {
        return Err("the stub answered with a completion".into());
    };
    assert_eq!(completed.render_id, render_id);
    Ok(())
}

#[tokio::test]
async fn the_broker_refuses_everything_outside_the_stanza() -> Result<(), Box<dyn std::error::Error>>
{
    let harness = Harness::start(true).await?;
    let publisher = harness.extractor().await?;

    // Another producer's fact is refused: a denied publish never receives an acknowledgement.
    let foreign = publisher
        .publish("evt.social.source.captured.v1", b"{}", "refused-1")
        .await;
    assert!(
        foreign.is_err(),
        "evt.social.source.captured.v1 must be refused"
    );

    // Another service's durable is invisible to this identity, while the administrator sees it.
    assert!(
        harness
            .admin
            .get_consumer_from_stream::<pull::Config, _, _>(FOREIGN_DURABLE, COMMANDS)
            .await
            .is_ok()
    );
    assert!(
        publisher
            .context()
            .get_consumer_from_stream::<pull::Config, _, _>(FOREIGN_DURABLE, COMMANDS)
            .await
            .is_err(),
        "CONSUMER.INFO on {FOREIGN_DURABLE} must be refused"
    );

    // Topology creation is refused: no stream, no consumer, no stream lookup.
    assert!(
        publisher
            .context()
            .create_stream(jetstream::stream::Config {
                name: "extractor_private".to_owned(),
                subjects: vec!["evt.extractor.private.>".to_owned()],
                ..jetstream::stream::Config::default()
            })
            .await
            .is_err()
    );
    assert!(
        publisher
            .context()
            .create_consumer_on_stream(
                pull::Config {
                    durable_name: Some("extractor_private".to_owned()),
                    ..pull::Config::default()
                },
                COMMANDS,
            )
            .await
            .is_err()
    );
    assert!(publisher.context().get_stream(COMMANDS).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_missing_or_different_durable_is_reported_and_never_created()
-> Result<(), Box<dyn std::error::Error>> {
    // Streams exist, the durables do not: Edge has not started yet.
    let harness = Harness::start(false).await?;
    let publisher = harness.extractor().await?;
    let database = TestDatabase::create().await?;

    let verification = verify_bus_topology(&publisher, CAPTURE_DURABLE).await;
    let message = verification
        .err()
        .ok_or("a missing durable must fail verification")?
        .to_string();
    assert!(message.contains(CAPTURE_DURABLE), "{message}");
    assert!(message.contains("ratatoskr-edge"), "{message}");
    let consumed = run_command_consumer(
        &publisher,
        database.database.pool(),
        CAPTURE_DURABLE,
        false,
        CancellationToken::new(),
    )
    .await;
    assert!(
        consumed.is_err(),
        "a missing durable must stop the consumer"
    );
    assert!(
        harness
            .admin
            .get_consumer_from_stream::<pull::Config, _, _>(CAPTURE_DURABLE, COMMANDS)
            .await
            .is_err(),
        "the extractor must not have created the durable"
    );

    // A durable that differs from the specification (ack wait) is refused as well.
    create_capture_durable(&harness.admin, Duration::from_mins(2)).await?;
    let consumed = run_command_consumer(
        &publisher,
        database.database.pool(),
        CAPTURE_DURABLE,
        false,
        CancellationToken::new(),
    )
    .await;
    assert!(
        consumed.is_err(),
        "a mismatched durable must stop the consumer"
    );
    database.cleanup().await?;
    Ok(())
}

async fn wait_for<F, Fut>(mut condition: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if condition().await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("the condition did not hold within 20 seconds".into())
}
