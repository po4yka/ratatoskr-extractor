//! Durable render-command consumption and completion deduplication.

use std::sync::Arc;

use async_nats::jetstream;
use browser_worker::{
    RenderExecutor, RenderOutcome, WorkerError, WorkerSettings, ensure_render_stream,
    run_render_consumer,
};
use futures_util::StreamExt as _;
use render_job::{
    NetworkEvidence, RENDER_COMPLETED_SUBJECT, RenderBudgets, RenderCommand, RenderCompleted,
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

fn nats_url() -> String {
    match std::env::var("EXTRACTOR_TEST_NATS_URL") {
        Ok(value) => value,
        Err(_) => "nats://127.0.0.1:4222".to_owned(),
    }
}

#[tokio::test]
async fn commands_are_consumed_once_and_deduped() -> Result<(), Box<dyn std::error::Error>> {
    let client = async_nats::connect(&nats_url()).await?;
    let context = jetstream::new(client.clone());
    let root = extractor_test_support::TemporaryBlobRoot::create().await?;
    let durable = format!("test_worker_{}", uuid::Uuid::now_v7().simple());
    let bucket = format!("completions_{}", uuid::Uuid::now_v7().simple());
    let settings = WorkerSettings {
        nats_url: nats_url(),
        blobs_root: root.path().to_path_buf(),
        durable_name: durable,
        completions_bucket: bucket,
    };
    // The shared command stream belongs to the fleet pipeline; the test provisions it.
    let _ = context
        .get_or_create_stream(async_nats::jetstream::stream::Config {
            name: "ratatoskr_commands".to_owned(),
            subjects: vec!["cmd.>".to_owned()],
            ..async_nats::jetstream::stream::Config::default()
        })
        .await?;
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
