//! Direct PDF runs through fetch, completion, and typed failure classes.

use async_nats::jetstream;
use extractor_blob_store::BlobStore;
use extractor_core::ExtractorConfig;
use extractor_eventing::{
    CompletedFetch, QueuedRun, claim_queued_run, complete_document, consume_capture,
    reject_quality, store_document_ir,
};
use extractor_pdf::{PdfDocumentInput, PdfError, PdfParseLimits, from_pdf};
use extractor_persistence::test_support::TestDatabase;
use extractor_safe_fetch::SafeFetcher;
use extractor_test_support::{ScriptedResponse, ScriptedServer, TemporaryBlobRoot};
use extractor_url_routing::RoutingPolicy;
use futures_util::stream;
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::DocumentId;
use serde_json::json;

const TEXT_PDF: &[u8] = include_bytes!("../../../crates/pdf/tests/fixtures/text-two-pages.pdf");
const ENCRYPTED_PDF: &[u8] =
    include_bytes!("../../../crates/pdf/tests/fixtures/encrypted-user-password.pdf");
const CORRUPT_PDF: &[u8] =
    include_bytes!("../../../crates/pdf/tests/fixtures/corrupt-truncated.pdf");
const NO_TEXT_PDF: &[u8] = include_bytes!("../../../crates/pdf/tests/fixtures/no-text-layer.pdf");

#[tokio::test]
async fn single_candidate_completion_commits_like_html() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let command_id = uuid::Uuid::now_v7();
    consume_capture(
        pool,
        "cmd.content.capture.requested.v1",
        &serde_json::to_vec(&capture_command(
            command_id,
            "https://example.test/report.pdf",
        ))?,
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the command did not produce queued work")?;

    let raw = store
        .store(
            "application/pdf",
            stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from_static(TEXT_PDF))]),
        )
        .await?;
    let extraction = from_pdf(
        PdfDocumentInput {
            document_id: run.document_id,
            source_address: DocumentAddress::parse(&run.url)?,
            source_blob: raw.clone(),
            bytes: TEXT_PDF,
        },
        generous_limits(),
    )?;
    let ir = store_document_ir(&store, &extraction.document).await?;
    let fetch = pdf_fetch(&run.url, &raw);
    complete_document(
        pool,
        run.run_id,
        &extraction.document,
        &ir,
        &fetch,
        &extraction.candidates,
    )
    .await?;

    let facts: (i64, i64, i64, i64, i64, String) = sqlx::query_as(
        "select
            (select count(*) from extractor.candidates where run_id = $1),
            (select count(*) from extractor.candidates where run_id = $1 and selected),
            (select count(*) from extractor.artifacts where run_id = $1),
            (select count(*) from extractor.artifacts where run_id = $1
                and kind = 'raw_source' and media_type = 'application/pdf'),
            (select count(*) from extractor.outbox_events
                where subject = 'evt.content.document.extracted.v1'),
            (select status from extractor.extraction_runs where run_id = $1)",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(facts, (1, 1, 2, 1, 1, "succeeded".to_owned()));
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn quality_rejection_records_explicit_class() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let command_id = uuid::Uuid::now_v7();
    consume_capture(
        pool,
        "cmd.content.capture.requested.v1",
        &serde_json::to_vec(&capture_command(
            command_id,
            "https://example.test/scanned.pdf",
        ))?,
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the command did not produce queued work")?;

    let raw = store
        .store(
            "application/pdf",
            stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from_static(
                NO_TEXT_PDF,
            ))]),
        )
        .await?;
    let Err(PdfError::NoTextLayer { candidates }) = from_pdf(
        PdfDocumentInput {
            document_id: run.document_id,
            source_address: DocumentAddress::parse(&run.url)?,
            source_blob: raw.clone(),
            bytes: NO_TEXT_PDF,
        },
        generous_limits(),
    ) else {
        return Err("the scanned fixture must degrade".into());
    };
    let fetch = pdf_fetch(&run.url, &raw);
    reject_quality(pool, run.run_id, &fetch, &candidates, "pdf_no_text_layer").await?;

    let outcome: (Option<String>, String) = sqlx::query_as(
        "select last_error_class, status from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        outcome,
        (Some("pdf_no_text_layer".to_owned()), "failed".to_owned())
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn pdf_classified_run_records_pdf_parser_version() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    consume_capture(
        pool,
        "cmd.content.capture.requested.v1",
        &serde_json::to_vec(&capture_command(
            uuid::Uuid::now_v7(),
            "https://example.test/paper.pdf",
        ))?,
    )
    .await?;
    consume_capture(
        pool,
        "cmd.content.capture.requested.v1",
        &serde_json::to_vec(&capture_command(
            uuid::Uuid::now_v7(),
            "https://example.test/article",
        ))?,
    )
    .await?;
    for url in [
        "https://news.ycombinator.com/item?id=900",
        "https://www.reddit.com/r/fixturerust/comments/abc123/fixture/",
    ] {
        consume_capture(
            pool,
            "cmd.content.capture.requested.v1",
            &serde_json::to_vec(&capture_command(uuid::Uuid::now_v7(), url))?,
        )
        .await?;
    }

    let versions: Vec<(String, String)> = sqlx::query_as(
        "select s.classification, r.parser_version
           from extractor.extraction_runs r
           join extractor.sources s on s.source_id = r.source_id
          order by s.classification",
    )
    .fetch_all(pool)
    .await?;
    assert_eq!(
        versions,
        vec![
            ("generic_web".to_owned(), "html-v1".to_owned()),
            ("hacker_news".to_owned(), "providers-v1".to_owned()),
            ("pdf".to_owned(), "pdf-v1".to_owned()),
            ("reddit".to_owned(), "providers-v1".to_owned()),
        ]
    );
    database.cleanup().await?;
    Ok(())
}

const REDDIT_JSON: &[u8] =
    include_bytes!("../../../crates/providers/tests/fixtures/reddit-post.json");
const REDDIT_CHALLENGE: &[u8] =
    include_bytes!("../../../crates/providers/tests/fixtures/reddit-challenge.html");

#[tokio::test]
async fn claimed_runs_carry_classification() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    queue_direct(
        pool,
        "https://news.ycombinator.com/item?id=900",
        "hacker_news",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;
    assert_eq!(
        run.classification, "hacker_news",
        "the claim must carry the source classification"
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn reddit_run_completes_from_json() -> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from_static(REDDIT_JSON)]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;

    // The mapped address keeps the production host, so the hermetic test drives the provider
    // boundary directly with its loopback equivalent.
    let address = server
        .uri("/r/fixturerust/comments/abc123/deterministic_extraction_fixture_post/.json")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        "https://www.reddit.com/r/fixturerust/comments/abc123/deterministic_extraction_fixture_post/",
        "reddit",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        extractor_providers::SourceRoute::Reddit,
        &address,
        &run,
    )
    .await?;

    let facts: (String, Option<String>, i64, i64, i64) = sqlx::query_as(
        "select
            r.status,
            r.last_error_class,
            (select count(*) from extractor.candidates c where c.run_id = r.run_id and c.selected
                and c.strategy = 'reddit_post'),
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
    assert_eq!(facts.1, None);
    assert_eq!(facts.2, 1, "one selected reddit_post candidate");
    assert_eq!(facts.3, 1);
    assert_eq!(facts.4, 1);
    assert_eq!(
        server.request_count(),
        1,
        "the native representation is fetched exactly once"
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn provider_non_json_fails_explicitly() -> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from_static(REDDIT_CHALLENGE)]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;

    let address = server
        .uri("/r/fixturerust/comments/abc123/deterministic_extraction_fixture_post/.json")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        "https://www.reddit.com/r/fixturerust/comments/abc123/deterministic_extraction_fixture_post/",
        "reddit",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        extractor_providers::SourceRoute::Reddit,
        &address,
        &run,
    )
    .await?;

    let outcome: (String, Option<String>) = sqlx::query_as(
        "select status, last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        outcome,
        ("failed".to_owned(), Some("provider_response".to_owned())),
        "an anti-bot challenge must fail as a typed provider response"
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn pdf_media_type_takes_direct_path_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from_static(TEXT_PDF)]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/pdf"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();

    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    // The scripted server binds loopback; a hostname keeps the public literal-address policy
    // applicable exactly as it is in production.
    let url = server.uri("/report.pdf").replace("127.0.0.1", "localhost");
    queue_direct(pool, &url, "pdf").await?;
    // The worker leases a run before processing it; terminal facts require that state.
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    let nats_client = async_nats::connect(&nats_url()).await?;
    process_pdf_run(
        pool,
        &fetcher,
        &store,
        &config,
        &jetstream::new(nats_client),
        &run,
    )
    .await?;

    let facts: (String, Option<String>, i64, i64, i64) = sqlx::query_as(
        "select
            r.status,
            r.last_error_class,
            (select count(*) from extractor.candidates c where c.run_id = r.run_id and c.selected
                and c.strategy = 'direct_pdf'),
            (select count(*) from extractor.artifacts a where a.run_id = r.run_id
                and a.kind = 'document_ir'),
            (select count(*) from extractor.outbox_events o
                where o.subject = 'evt.content.document.extracted.v1'
                and o.payload->'aggregate_id' is not null)
           from extractor.extraction_runs r where r.run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(facts.0, "succeeded");
    assert_eq!(facts.1, None);
    assert_eq!(facts.2, 1);
    assert_eq!(facts.3, 1);
    assert_eq!(facts.4, 1);
    assert_eq!(server.request_count(), 1, "the URL is fetched exactly once");
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn pdf_failure_classes_reach_terminal_state() -> Result<(), Box<dyn std::error::Error>> {
    for (fixture, expected_class) in [(ENCRYPTED_PDF, "pdf_encrypted"), (CORRUPT_PDF, "parse")] {
        let server = ScriptedServer::start(vec![
            ScriptedResponse::chunks([bytes::Bytes::from_static(fixture)]).with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/pdf"),
            ),
        ])
        .await?;
        let database = TestDatabase::create().await?;
        let root = TemporaryBlobRoot::create().await?;
        let store = BlobStore::new(root.path());
        let pool = database.database.pool();
        let mut config = ExtractorConfig::built_in(root.path());
        config.fetch.allowed_ports = vec![server.port()];
        let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
        // A hostname keeps the public literal-address policy applicable exactly as in production.
        queue_direct(
            pool,
            &server
                .uri("/document.pdf")
                .replace("127.0.0.1", "localhost"),
            "pdf",
        )
        .await?;
        // The worker leases a run before processing it; terminal facts require that state.
        let run = claim_queued_run(pool, "test-worker", 60)
            .await?
            .ok_or("the queued run did not lease")?;

        let nats_client = async_nats::connect(&nats_url()).await?;
        process_pdf_run(
            pool,
            &fetcher,
            &store,
            &config,
            &jetstream::new(nats_client),
            &run,
        )
        .await?;

        let outcome: (String, Option<String>) = sqlx::query_as(
            "select status, last_error_class from extractor.extraction_runs where run_id = $1",
        )
        .bind(run.run_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(
            outcome,
            ("failed".to_owned(), Some(expected_class.to_owned())),
            "fixture must fail with its typed class"
        );
        database.cleanup().await?;
    }
    Ok(())
}

async fn process_pdf_run(
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
    let document_id = DocumentId::new_v7().0;
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

fn capture_command(command_id: uuid::Uuid, url: &str) -> serde_json::Value {
    let operation_id = uuid::Uuid::now_v7();
    json!({
        "command_id": command_id,
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-22T10:00:00Z",
        "operation_id": operation_id,
        "tenant_id": format!("user:{}", uuid::Uuid::now_v7()),
        "correlation_id": format!("operation:{operation_id}"),
        "idempotency_key": "capture-pdf",
        "payload": { "url": url }
    })
}

fn pdf_fetch<'a>(
    final_url: &'a str,
    raw: &'a ratatoskr_identifiers::BlobRef,
) -> CompletedFetch<'a> {
    CompletedFetch {
        final_url,
        http_status: 200,
        media_type: "application/pdf",
        wire_bytes: raw.length_bytes,
        decoded_bytes: raw.length_bytes,
        attempts: 1,
        cache_outcome: "fresh",
        etag: None,
        last_modified: None,
        raw_blob: raw,
    }
}

fn generous_limits() -> PdfParseLimits {
    PdfParseLimits {
        max_input_bytes: 65_536,
        max_pages: 100,
        max_text_bytes: 1_048_576,
    }
}

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
    };
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

    process_pdf_run(pool, &fetcher, &store, &config, &bus, &run).await?;

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
    assert_eq!(
        server.request_count(),
        2,
        "the shell is fetched directly and the hydrated page by the worker"
    );

    worker_cancel.cancel();
    worker_task.await??;
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
