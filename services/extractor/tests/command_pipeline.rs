//! Capture command through durable completion and acknowledged publication.

use std::time::Duration;

use extractor_blob_store::BlobStore;
use extractor_document_ir::{DocumentIrError, HtmlDocumentInput, ParseLimits, from_html};
use extractor_eventing::{
    CompletedFetch, NatsPublisher, Publisher, claim_queued_run, complete_document, consume_capture,
    reject_quality, run_command_consumer, run_outbox_once, store_document_ir,
};
use extractor_persistence::test_support::TestDatabase;
use extractor_test_support::TemporaryBlobRoot;
use futures_util::stream;
use ratatoskr_document_contracts::DocumentAddress;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn completed_html_persists_all_candidate_decisions_atomically()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let publisher = NatsPublisher::connect(&nats_url()).await?;
    publisher.ensure_command_stream().await?;
    publisher.ensure_event_stream().await?;
    let operation_id = uuid::Uuid::now_v7();
    let command_id = uuid::Uuid::now_v7();
    let durable = format!("extractor_test_{}", uuid::Uuid::now_v7().simple());
    let command = serde_json::to_vec(&json!({
        "command_id": command_id,
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-21T10:00:00Z",
        "operation_id": operation_id,
        "tenant_id": format!("user:{}", uuid::Uuid::now_v7()),
        "correlation_id": format!("operation:{operation_id}"),
        "idempotency_key": "capture-vertical",
        "payload": { "url": "https://example.test/article" }
    }))?;
    for delivery in ["first", "redelivery"] {
        publisher
            .publish(
                "cmd.content.capture.requested.v1",
                &command,
                &format!("{command_id}-{delivery}"),
            )
            .await?;
    }
    let cancellation = CancellationToken::new();
    let consumer = tokio::spawn({
        let publisher = publisher.clone();
        let pool = database.database.pool().clone();
        let durable = durable.clone();
        let cancellation = cancellation.clone();
        async move { run_command_consumer(&publisher, &pool, &durable, cancellation).await }
    });
    wait_for_run(database.database.pool()).await?;
    cancellation.cancel();
    consumer.await??;

    let run = claim_queued_run(database.database.pool(), "test-worker", 60)
        .await?
        .ok_or("the command did not produce queued work")?;
    let html = bytes::Bytes::from_static(
        b"<!doctype html><html><head><title>Article</title></head><body><article><h1>Article</h1><p>This accepted article contains enough deterministic content to cross the minimum quality boundary without relying on page chrome.</p><p>A second paragraph provides stable context and evidence for the command pipeline result.</p></article></body></html>",
    );
    let raw = store
        .store(
            "text/html",
            stream::iter([Ok::<_, std::io::Error>(html.clone())]),
        )
        .await?;
    let extraction = from_html(
        HtmlDocumentInput {
            document_id: run.document_id,
            source_address: DocumentAddress::parse(&run.url)?,
            source_blob: raw.clone(),
            bytes: &html,
        },
        ParseLimits {
            max_input_bytes: 4_096,
            max_dom_nodes: 64,
        },
    )?;
    let document = extraction.document;
    let ir = store_document_ir(&store, &document).await?;
    let fetch = CompletedFetch {
        final_url: &run.url,
        http_status: 200,
        media_type: "text/html",
        wire_bytes: u64::try_from(html.len())?,
        decoded_bytes: u64::try_from(html.len())?,
        attempts: 1,
        cache_outcome: "fresh",
        etag: None,
        last_modified: None,
        raw_blob: &raw,
    };
    complete_document(
        database.database.pool(),
        run.run_id,
        &document,
        &ir,
        &fetch,
        &extraction.candidates,
    )
    .await?;
    let report =
        run_outbox_once(database.database.pool(), &publisher, "test-publisher", 10).await?;
    assert_eq!(report.published, 3);

    verify_candidate_completion(database.database.pool(), run.run_id, &document).await?;

    database.cleanup().await?;
    Ok(())
}

async fn verify_candidate_completion(
    pool: &sqlx::PgPool,
    run_id: uuid::Uuid,
    document: &ratatoskr_document_contracts::Document,
) -> Result<(), Box<dyn std::error::Error>> {
    let events: (i64, i64, i64) = sqlx::query_as(
        "select
            count(*) filter (where subject = 'evt.content.document.extracted.v1'),
            count(*) filter (where subject = 'evt.platform.operation.reported.v1'
                              and payload->'payload'->>'status' = 'succeeded'),
            count(*) filter (where published_at is null)
           from extractor.outbox_events",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(events, (1, 1, 0));
    let inbox_count: i64 = sqlx::query_scalar("select count(*) from extractor.inbox_events")
        .fetch_one(pool)
        .await?;
    assert_eq!(inbox_count, 1);
    let facts: (i64, i64, i64, i64, String) = sqlx::query_as(
        "select count(*), count(*) filter (where selected), count(score),
                (select count(*) from extractor.artifacts where run_id = $1),
                (select status from extractor.extraction_runs where run_id = $1)
           from extractor.candidates where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(facts, (3, 1, 3, 2, "succeeded".to_owned()));
    let selected: String = sqlx::query_scalar(
        "select strategy from extractor.candidates where run_id = $1 and selected",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        document
            .provenance
            .first()
            .map(|entry| entry.extraction_strategy.as_str()),
        Some(selected.as_str())
    );
    Ok(())
}

#[tokio::test]
async fn quality_rejection_persists_evidence_without_document_event()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let operation_id = uuid::Uuid::now_v7();
    let command_id = uuid::Uuid::now_v7();
    let command = serde_json::to_vec(&json!({
        "command_id": command_id,
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-21T10:00:00Z",
        "operation_id": operation_id,
        "tenant_id": format!("user:{}", uuid::Uuid::now_v7()),
        "correlation_id": format!("operation:{operation_id}"),
        "idempotency_key": "capture-low-quality",
        "payload": { "url": "https://example.test/login" }
    }))?;
    consume_capture(
        database.database.pool(),
        "cmd.content.capture.requested.v1",
        &command,
    )
    .await?;
    let run = claim_queued_run(database.database.pool(), "test-worker", 60)
        .await?
        .ok_or("the command did not produce queued work")?;
    let html = bytes::Bytes::from_static(
        b"<html><head><title>Sign in</title></head><body><main class='login'><h1>Sign in</h1><p>Accept cookies to continue.</p></main></body></html>",
    );
    let raw = store
        .store(
            "text/html",
            stream::iter([Ok::<_, std::io::Error>(html.clone())]),
        )
        .await?;
    let result = from_html(
        HtmlDocumentInput {
            document_id: run.document_id,
            source_address: DocumentAddress::parse(&run.url)?,
            source_blob: raw.clone(),
            bytes: &html,
        },
        ParseLimits {
            max_input_bytes: 4_096,
            max_dom_nodes: 64,
        },
    );
    let Err(DocumentIrError::LowQuality { candidates }) = result else {
        return Err("login shell did not produce low-quality evidence".into());
    };
    let fetch = CompletedFetch {
        final_url: &run.url,
        http_status: 200,
        media_type: "text/html",
        wire_bytes: u64::try_from(html.len())?,
        decoded_bytes: u64::try_from(html.len())?,
        attempts: 1,
        cache_outcome: "fresh",
        etag: None,
        last_modified: None,
        raw_blob: &raw,
    };
    reject_quality(database.database.pool(), run.run_id, &fetch, &candidates).await?;

    let facts: (i64, i64, i64, i64, i64, String) = sqlx::query_as(
        "select
            (select count(*) from extractor.candidates where run_id = $1),
            (select count(*) from extractor.candidates where run_id = $1 and selected),
            (select count(*) from extractor.artifacts where run_id = $1 and kind = 'raw_source'),
            (select count(*) from extractor.artifacts where run_id = $1 and kind = 'document_ir'),
            (select count(*) from extractor.outbox_events where operation_id = $2 and subject = 'evt.content.document.extracted.v1'),
            (select status from extractor.extraction_runs where run_id = $1)",
    )
    .bind(run.run_id)
    .bind(operation_id)
    .fetch_one(database.database.pool())
    .await?;
    assert_eq!(facts, (3, 0, 1, 0, 0, "failed".to_owned()));
    let failed_reports: i64 = sqlx::query_scalar(
        "select count(*) from extractor.outbox_events where operation_id = $1 and payload->'payload'->>'status' = 'failed'",
    )
    .bind(operation_id)
    .fetch_one(database.database.pool())
    .await?;
    assert_eq!(failed_reports, 1);
    database.cleanup().await?;
    Ok(())
}

async fn wait_for_run(pool: &sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let count: i64 = sqlx::query_scalar("select count(*) from extractor.extraction_runs")
                .fetch_one(pool)
                .await?;
            if count == 1 {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    Ok(())
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
