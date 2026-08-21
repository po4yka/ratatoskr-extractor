//! Capture command through durable completion and acknowledged publication.

use std::time::Duration;

use extractor_blob_store::BlobStore;
use extractor_document_ir::{HtmlDocumentInput, ParseLimits, from_html};
use extractor_eventing::{
    CompletedFetch, NatsPublisher, Publisher, claim_queued_run, complete_document,
    run_command_consumer, run_outbox_once, store_document_ir,
};
use extractor_persistence::test_support::TestDatabase;
use extractor_test_support::TemporaryBlobRoot;
use futures_util::stream;
use ratatoskr_document_contracts::DocumentAddress;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn consumed_capture_publishes_one_document_and_one_report()
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
        b"<!doctype html><html><head><title>Article</title></head><body><h1>One</h1><p>Body.</p></body></html>",
    );
    let raw = store
        .store(
            "text/html",
            stream::iter([Ok::<_, std::io::Error>(html.clone())]),
        )
        .await?;
    let document = from_html(
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
    complete_document(database.database.pool(), run.run_id, &document, &ir, &fetch).await?;
    let report =
        run_outbox_once(database.database.pool(), &publisher, "test-publisher", 10).await?;
    assert_eq!(report.published, 3);

    let counts: (i64, i64, i64) = sqlx::query_as(
        "select
            count(*) filter (where subject = 'evt.content.document.extracted.v1'),
            count(*) filter (where subject = 'evt.platform.operation.reported.v1'
                              and payload->'payload'->>'status' = 'succeeded'),
            count(*) filter (where published_at is null)
           from extractor.outbox_events",
    )
    .fetch_one(database.database.pool())
    .await?;
    assert_eq!(counts, (1, 1, 0));
    let inbox_count: i64 = sqlx::query_scalar("select count(*) from extractor.inbox_events")
        .fetch_one(database.database.pool())
        .await?;
    assert_eq!(inbox_count, 1);

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
