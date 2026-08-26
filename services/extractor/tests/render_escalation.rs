//! Browser escalation through the gated policy against a real worker stack.

use async_nats::jetstream;
use extractor_blob_store::BlobStore;
use extractor_core::ExtractorConfig;
use extractor_eventing::{QueuedRun, claim_queued_run};
use extractor_persistence::test_support::TestDatabase;
use extractor_safe_fetch::SafeFetcher;
use extractor_test_support::TemporaryBlobRoot;
use extractor_url_routing::RoutingPolicy;

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
#[allow(
    clippy::large_futures,
    reason = "the hermetic scenario holds one full resource set per test future"
)]
async fn empty_shell_escalates_and_completes_from_rendered_dom()
-> Result<(), Box<dyn std::error::Error>> {
    const SHELL: &[u8] =
        b"<html><body><div id=\"root\"></div><script src=\"/app.js\"></script></body></html>";
    const RENDERED: &[u8] = b"<html><body><div id=\"root\"><p>Hydrated fixture content for the escalated run carries more than the threshold of deterministic prose so the shared evaluator accepts it.</p></div></body></html>";
    let server = extractor_test_support::ScriptedServer::start(vec![
        extractor_test_support::ScriptedResponse::chunks([bytes::Bytes::from_static(SHELL)])
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/html"),
            ),
        extractor_test_support::ScriptedResponse::chunks([bytes::Bytes::from_static(RENDERED)])
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/html"),
            ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let worker_root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let nats_client = async_nats::connect(&nats_url()).await?;
    let bus = jetstream::new(nats_client.clone());

    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![80, 443, server.port()];
    config.render.enabled = true;
    config.render.worker_blobs_root = worker_root.path().to_path_buf();
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;

    // The browser worker runs as an independent component against the same bus.
    let chrome = chrome_bin_for_worker();
    let worker_cancel = tokio_util::sync::CancellationToken::new();
    let executor = browser_worker::ChromiumExecutor::launch_with_policy(
        Some(chrome),
        browser_worker::NavigationPolicy {
            routing: RoutingPolicy {
                max_url_length: 8_192,
                allowed_ports: vec![80, 443, server.port()],
            },
            allow_loopback: true,
        },
    )
    .await?;
    let worker_settings = browser_worker::WorkerSettings {
        nats_url: nats_url(),
        blobs_root: worker_root.path().to_path_buf(),
        durable_name: format!("test_worker_{}", uuid::Uuid::now_v7().simple()),
        completions_bucket: format!("completions_{}", uuid::Uuid::now_v7().simple()),
        max_jobs_per_process: u32::MAX,
    };
    let events_publisher = extractor_eventing::NatsPublisher::connect(&nats_url()).await?;
    events_publisher.ensure_event_stream().await?;
    let worker_task = tokio::spawn(browser_worker::run_render_consumer(
        jetstream::new(nats_client.clone()),
        worker_settings,
        executor,
        worker_cancel.clone(),
    ));

    // A hostname keeps the public literal-address policy applicable exactly as in production.
    queue_direct(
        pool,
        &server.uri("/page").replace("127.0.0.1", "localhost"),
        "generic_web",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    process_run(pool, &fetcher, &store, &config, &bus, &run).await?;

    let class: Option<String> = sqlx::query_scalar(
        "select last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    eprintln!("DIAG-TEST escalate class={class:?}");

    let facts: (String, i64, i64) = sqlx::query_as(
        "select
            r.status,
            (select count(*) from extractor.artifacts a where a.run_id = r.run_id
                and a.kind = 'document_ir'),
            (select count(*) from extractor.outbox_events o
                where o.subject = 'evt.content.document.extracted.v1')
           from extractor.extraction_runs r where r.run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(facts.0, "succeeded");
    assert_eq!(facts.1, 1);
    assert_eq!(facts.2, 1);
    // Only the two scripted navigations count: current Chrome builds issue extra
    // incidental subresource requests (favicon and friends) that the worker denies,
    // so raw request totals are not hermetic across browser releases.
    let page_navigations = server
        .requests()
        .await
        .iter()
        .filter(|request| request.path_and_query.starts_with("/page"))
        .count();
    assert_eq!(
        page_navigations, 2,
        "the shell is fetched directly and the hydrated page by the worker"
    );

    worker_cancel.cancel();
    worker_task.await??;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[allow(
    clippy::large_futures,
    reason = "the hermetic scenario holds one full resource set per test future"
)]
async fn host_outside_the_allowlist_denies_without_rendering()
-> Result<(), Box<dyn std::error::Error>> {
    const SHELL: &[u8] = b"<html><body><div id=\"root\"></div></body></html>";
    let server = extractor_test_support::ScriptedServer::start(vec![
        extractor_test_support::ScriptedResponse::chunks([bytes::Bytes::from_static(SHELL)])
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/html"),
            ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = extractor_test_support::TemporaryBlobRoot::create().await?;
    let worker_root = extractor_test_support::TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let nats_client = async_nats::connect(&nats_url()).await?;
    let bus = jetstream::new(nats_client.clone());

    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![80, 443, server.port()];
    config.render.enabled = true;
    config.render.allowed_hosts = vec!["allowed.example".to_owned()];
    config.render.total_timeout_ms = 2_000;
    config.render.worker_blobs_root = worker_root.path().to_path_buf();
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;

    queue_direct(
        pool,
        &server.uri("/page").replace("127.0.0.1", "localhost"),
        "generic_web",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    process_run(pool, &fetcher, &store, &config, &bus, &run).await?;

    let class: Option<String> = sqlx::query_scalar(
        "select last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(class.as_deref(), Some("quality"));

    let step: Option<(String, Option<String>)> = sqlx::query_as(
        "select kind, outcome from extractor.provider_resolutions
          where run_id = $1 and kind = 'render_policy'",
    )
    .bind(run.run_id)
    .fetch_optional(pool)
    .await?;
    assert_eq!(
        step,
        Some((
            "render_policy".to_owned(),
            Some("host_not_allowed".to_owned())
        )),
        "the denial must name the refusing gate"
    );

    assert_eq!(
        server.request_count(),
        1,
        "a denied escalation must fetch directly and never reach a worker"
    );
    let counter: i32 = sqlx::query_scalar(
        "select coalesce((select escalated from extractor.render_budgets
                          where utc_day = current_date), 0)",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(counter, 0, "the day budget must stay untouched on denial");

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[allow(
    clippy::large_futures,
    reason = "the hermetic scenario holds one full resource set per test future"
)]
async fn exhausted_daily_budget_denies_without_rendering() -> Result<(), Box<dyn std::error::Error>>
{
    const SHELL: &[u8] = b"<html><body><div id=\"root\"></div></body></html>";
    let server = extractor_test_support::ScriptedServer::start(vec![
        extractor_test_support::ScriptedResponse::chunks([bytes::Bytes::from_static(SHELL)])
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/html"),
            ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = extractor_test_support::TemporaryBlobRoot::create().await?;
    let worker_root = extractor_test_support::TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let nats_client = async_nats::connect(&nats_url()).await?;
    let bus = jetstream::new(nats_client.clone());

    sqlx::query(
        "insert into extractor.render_budgets (utc_day, escalated) values (current_date, 7)",
    )
    .execute(pool)
    .await?;

    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![80, 443, server.port()];
    config.render.enabled = true;
    config.render.max_escalations_per_day = 7;
    config.render.total_timeout_ms = 2_000;
    config.render.worker_blobs_root = worker_root.path().to_path_buf();
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;

    queue_direct(
        pool,
        &server.uri("/page").replace("127.0.0.1", "localhost"),
        "generic_web",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    process_run(pool, &fetcher, &store, &config, &bus, &run).await?;

    let class: Option<String> = sqlx::query_scalar(
        "select last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(class.as_deref(), Some("quality"));

    let step: Option<(String, Option<String>)> = sqlx::query_as(
        "select kind, outcome from extractor.provider_resolutions
          where run_id = $1 and kind = 'render_policy'",
    )
    .bind(run.run_id)
    .fetch_optional(pool)
    .await?;
    assert_eq!(
        step,
        Some((
            "render_policy".to_owned(),
            Some("daily_budget_exhausted".to_owned())
        )),
        "an exhausted budget must deny with its own gate"
    );

    assert_eq!(
        server.request_count(),
        1,
        "an exhausted budget must never reach a worker"
    );
    let counter: i32 = sqlx::query_scalar(
        "select escalated from extractor.render_budgets where utc_day = current_date",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(counter, 7, "a denial must not advance the counter");

    database.cleanup().await?;
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only browser location is not process configuration"
)]
fn chrome_bin_for_worker() -> String {
    match std::env::var("CHROME_BIN") {
        Ok(value) => value,
        Err(_) => "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_owned(),
    }
}

#[allow(
    clippy::large_futures,
    reason = "the hermetic scenario holds one full resource set per test future"
)]
async fn process_run(
    pool: &sqlx::PgPool,
    fetcher: &SafeFetcher,
    store: &BlobStore,
    config: &ExtractorConfig,
    bus: &async_nats::jetstream::Context,
    run: &QueuedRun,
) -> Result<(), Box<dyn std::error::Error>> {
    extractor_service::process_run(
        pool,
        fetcher,
        store,
        &config.parser,
        &config.pdf,
        &config.providers,
        &config.render,
        &config.youtube,
        bus,
        run,
    )
    .await?;
    Ok(())
}

async fn queue_direct(
    pool: &sqlx::PgPool,
    url: &str,
    classification: &str,
) -> Result<(), sqlx::Error> {
    let command_id = uuid::Uuid::now_v7();
    let operation_id = uuid::Uuid::now_v7();
    let owner_id = uuid::Uuid::now_v7();
    let source_id = uuid::Uuid::now_v7();
    let run_id = uuid::Uuid::now_v7();
    let document_id = ratatoskr_identifiers::DocumentId::new_v7().0;
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', ':']).next())
        .unwrap_or("localhost")
        .to_owned();
    sqlx::query(
        "insert into extractor.inbox_events (command_id, subject, command_type, producer, received_at)
         values ($1, 'cmd.content.capture.requested.v1', 'content.capture.requested.v1',
                 'ratatoskr-platform', transaction_timestamp())",
    )
    .bind(command_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.sources
             (source_id, owner_id, original_url, normalized_url, canonical_url, host,
              classification, created_at)
          values ($1, $2, $3, $3, $3, $4, $5, transaction_timestamp())",
    )
    .bind(source_id)
    .bind(owner_id)
    .bind(url)
    .bind(host)
    .bind(classification)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.extraction_runs
             (run_id, command_id, operation_id, owner_id, correlation_id, source_id, document_id,
              status, policy_version, normalizer_version, parser_version, queued_at)
          values ($1, $2, $3, $4, $5, $6, $7, 'queued', 'ssrf-v1', 'url-v1', 'html-v1',
                  transaction_timestamp())",
    )
    .bind(run_id)
    .bind(command_id)
    .bind(operation_id)
    .bind(owner_id)
    .bind(format!("operation:{operation_id}"))
    .bind(source_id)
    .bind(document_id)
    .execute(pool)
    .await?;
    Ok(())
}
