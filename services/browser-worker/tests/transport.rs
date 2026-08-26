//! Durable render-command consumption and completion deduplication.

use std::sync::Arc;

use async_nats::jetstream;
use browser_worker::{
    RenderExecutor, RenderOutcome, WorkerError, WorkerSettings, ensure_render_stream,
    run_render_consumer,
};
use futures_util::StreamExt as _;
use render_job::{
    NetworkEvidence, RENDER_COMPLETED_SUBJECT, RENDER_REQUESTED_SUBJECT, RenderBudgets,
    RenderCommand, RenderCompleted,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct RecordingExecutor {
    invocations: Mutex<Vec<RenderCommand>>,
}

impl RenderExecutor for RecordingExecutor {
    async fn render(&self, command: &RenderCommand) -> Result<RenderOutcome, WorkerError> {
        self.invocations.lock().await.push(command.clone());
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
        render_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000ba"),
        operation_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000bb"),
        correlation_id: "operation:test".to_owned(),
        tenant_user_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000bc"),
        url: "https://example.test/app".to_owned(),
        budgets: RenderBudgets {
            navigation_timeout_ms: 5_000,
            total_timeout_ms: 10_000,
            max_dom_bytes: 65_536,
        },
    }
}

/// One test owns the shared fleet stream at a time: every consumer matches the
/// same render subject, so concurrent scenarios would steal each other's
/// deliveries.
static BUS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[expect(
    clippy::disallowed_methods,
    reason = "test-only broker location is not process configuration"
)]
fn nats_url() -> String {
    match std::env::var("EXTRACTOR_TEST_NATS_URL") {
        Ok(value) => value,
        Err(_) => "nats://127.0.0.1:4222".to_owned(),
    }
}

#[tokio::test]
async fn commands_are_consumed_once_and_deduped() -> Result<(), Box<dyn std::error::Error>> {
    let _bus = BUS_LOCK.lock().await;
    let client = async_nats::connect(&nats_url()).await?;
    let context = jetstream::new(client.clone());
    let root = extractor_test_support::TemporaryBlobRoot::create().await?;
    let durable = format!("test_worker_{}", uuid::Uuid::now_v7().simple());
    let bucket = format!("completions_{}", uuid::Uuid::now_v7().simple());
    let settings = WorkerSettings {
        nats_url: nats_url(),
        chrome_bin: None,
        blobs_root: root.path().to_path_buf(),
        durable_name: durable,
        completions_bucket: bucket,
        max_jobs_per_process: u32::MAX,
    };
    // The shared command stream belongs to the fleet pipeline; the test provisions it.
    let _ = context
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: "ratatoskr_commands".to_owned(),
            subjects: vec!["cmd.>".to_owned()],
            ..async_nats::jetstream::stream::Config::default()
        })
        .await?;
    // The shared stream accumulates deliveries from earlier runs; this test owns a fresh
    // durable consumer and starts from an empty log.
    let command_stream = context
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: "ratatoskr_commands".to_owned(),
            subjects: vec!["cmd.>".to_owned()],
            ..async_nats::jetstream::stream::Config::default()
        })
        .await?;
    command_stream.purge().await?;
    let events_publisher = extractor_eventing::NatsPublisher::connect(&nats_url()).await?;
    events_publisher.ensure_event_stream().await?;
    ensure_render_stream(&context, &settings.completions_bucket).await?;

    let executor = Arc::new(RecordingExecutor::default());
    let cancellation = CancellationToken::new();
    let worker = tokio::spawn(run_render_consumer(
        context.clone(),
        settings,
        RecordingExecutorProxy(executor.clone()),
        cancellation.clone(),
    ));

    let payload = serde_json::to_vec(&command())?;
    // Commands enter the pipeline through JetStream publication.
    context
        .publish(render_job::RENDER_REQUESTED_SUBJECT, payload.clone().into())
        .await?
        .await?;
    let _ = client.flush().await;

    // Subscribe before the worker runs: core-NATS subscribers have no history.
    let mut completions = client.subscribe(RENDER_COMPLETED_SUBJECT).await?;
    let completed: RenderCompleted =
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let message = completions.next().await.expect("completion arrives");
                let decoded: RenderCompleted = serde_json::from_slice(&message.payload)?;
                if decoded.render_id == command().render_id {
                    return Ok::<_, Box<dyn std::error::Error>>(decoded);
                }
            }
        })
        .await??;
    assert_eq!(completed.final_url, "https://example.test/app");
    assert_eq!(
        completed.dom.owner_service.as_str(),
        "ratatoskr-browser-worker"
    );

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        executor.invocations.lock().await.len(),
        1,
        "the first delivery executes once"
    );

    // A redelivery of the same command must not render again.
    context
        .publish(render_job::RENDER_REQUESTED_SUBJECT, payload.into())
        .await?
        .await?;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        executor.invocations.lock().await.len(),
        1,
        "the dedup bucket absorbs the redelivery"
    );

    cancellation.cancel();
    worker.await??;
    Ok(())
}

struct RecordingExecutorProxy(Arc<RecordingExecutor>);

impl RenderExecutor for RecordingExecutorProxy {
    async fn render(&self, command: &RenderCommand) -> Result<RenderOutcome, WorkerError> {
        self.0.render(command).await
    }
}

/// Fails every command with a stable class, recording invocations.
#[derive(Default)]
struct FailingExecutor {
    invocations: Mutex<Vec<RenderCommand>>,
}

impl RenderExecutor for FailingExecutor {
    async fn render(&self, command: &RenderCommand) -> Result<RenderOutcome, WorkerError> {
        self.invocations.lock().await.push(command.clone());
        Err(WorkerError::Failed(
            render_job::RenderFailureClass::NavigationTimeout,
        ))
    }
}

async fn provision_fresh_stream(
    context: &jetstream::Context,
) -> Result<(), Box<dyn std::error::Error>> {
    let _ = context
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: "ratatoskr_commands".to_owned(),
            subjects: vec!["cmd.>".to_owned()],
            ..async_nats::jetstream::stream::Config::default()
        })
        .await?;
    context
        .get_stream("ratatoskr_commands")
        .await?
        .purge()
        .await?;
    Ok(())
}

fn variant_command(render_id: uuid::Uuid) -> RenderCommand {
    RenderCommand {
        render_id,
        ..command()
    }
}

#[tokio::test]
async fn consumer_exits_after_the_configured_job_count() -> Result<(), Box<dyn std::error::Error>> {
    let _bus = BUS_LOCK.lock().await;
    let client = async_nats::connect(&nats_url()).await?;
    let context = jetstream::new(client.clone());
    let root = extractor_test_support::TemporaryBlobRoot::create().await?;
    let settings = WorkerSettings {
        nats_url: nats_url(),
        chrome_bin: None,
        blobs_root: root.path().to_path_buf(),
        durable_name: format!("test_worker_{}", uuid::Uuid::now_v7().simple()),
        completions_bucket: format!("completions_{}", uuid::Uuid::now_v7().simple()),
        max_jobs_per_process: 2,
    };
    provision_fresh_stream(&context).await?;
    let events_publisher = extractor_eventing::NatsPublisher::connect(&nats_url()).await?;
    events_publisher.ensure_event_stream().await?;
    ensure_render_stream(&context, &settings.completions_bucket).await?;

    let executor = Arc::new(RecordingExecutor::default());
    let worker = tokio::spawn(run_render_consumer(
        context.clone(),
        settings.clone(),
        RecordingExecutorProxy(executor.clone()),
        CancellationToken::new(),
    ));

    // Subscribe before the worker runs: core-NATS subscribers have no history.
    let mut completions = client.subscribe(RENDER_COMPLETED_SUBJECT).await?;
    for id in [
        uuid::uuid!("018f0000-0000-7000-8000-0000000000c1"),
        uuid::uuid!("018f0000-0000-7000-8000-0000000000c2"),
    ] {
        let payload = serde_json::to_vec(&variant_command(id))?;
        context
            .publish(RENDER_REQUESTED_SUBJECT, payload.into())
            .await?
            .await?;
    }

    // Both terminal outcomes publish before the process budget ends the loop.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut seen = 0;
        while seen < 2 {
            let message = completions.next().await.expect("completion arrives");
            let _: RenderCompleted = serde_json::from_slice(&message.payload)?;
            seen += 1;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;

    // The consumer future itself resolves once two jobs reached a terminal state.
    let exited = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
    assert!(
        exited.is_ok(),
        "the consumer must return after its job budget"
    );
    exited.expect("consumer returned before the deadline")??;

    let invocations = executor.invocations.lock().await.len();
    assert_eq!(invocations, 2, "each job rendered exactly once");
    Ok(())
}

#[tokio::test]
async fn failed_jobs_count_toward_recycling() -> Result<(), Box<dyn std::error::Error>> {
    let _bus = BUS_LOCK.lock().await;
    let client = async_nats::connect(&nats_url()).await?;
    let context = jetstream::new(client.clone());
    let root = extractor_test_support::TemporaryBlobRoot::create().await?;
    let settings = WorkerSettings {
        nats_url: nats_url(),
        chrome_bin: None,
        blobs_root: root.path().to_path_buf(),
        durable_name: format!("test_worker_{}", uuid::Uuid::now_v7().simple()),
        completions_bucket: format!("completions_{}", uuid::Uuid::now_v7().simple()),
        max_jobs_per_process: 2,
    };
    provision_fresh_stream(&context).await?;
    let events_publisher = extractor_eventing::NatsPublisher::connect(&nats_url()).await?;
    events_publisher.ensure_event_stream().await?;
    ensure_render_stream(&context, &settings.completions_bucket).await?;

    let executor = Arc::new(FailingExecutor::default());
    let mut failures = client.subscribe(render_job::RENDER_FAILED_SUBJECT).await?;
    let worker = tokio::spawn(run_render_consumer(
        context.clone(),
        settings,
        FailingExecutorProxy(executor.clone()),
        CancellationToken::new(),
    ));

    for id in [
        uuid::uuid!("018f0000-0000-7000-8000-0000000000d1"),
        uuid::uuid!("018f0000-0000-7000-8000-0000000000d2"),
    ] {
        let payload = serde_json::to_vec(&variant_command(id))?;
        context
            .publish(RENDER_REQUESTED_SUBJECT, payload.into())
            .await?
            .await?;
    }

    let failing_ids = [
        uuid::uuid!("018f0000-0000-7000-8000-0000000000d1"),
        uuid::uuid!("018f0000-0000-7000-8000-0000000000d2"),
    ];
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut seen = 0;
        while seen < 2 {
            let message = failures.next().await.expect("failure arrives");
            let decoded: render_job::RenderFailed = serde_json::from_slice(&message.payload)?;
            assert_eq!(
                decoded.class,
                render_job::RenderFailureClass::NavigationTimeout
            );
            if failing_ids.contains(&decoded.render_id) {
                seen += 1;
            }
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;

    let exited = tokio::time::timeout(std::time::Duration::from_secs(10), worker).await;
    assert!(
        exited.is_ok(),
        "failed terminal outcomes must count toward the job budget"
    );
    exited.expect("worker returned before the deadline")??;
    Ok(())
}

struct FailingExecutorProxy(Arc<FailingExecutor>);

impl RenderExecutor for FailingExecutorProxy {
    async fn render(&self, command: &RenderCommand) -> Result<RenderOutcome, WorkerError> {
        self.0.render(command).await
    }
}
