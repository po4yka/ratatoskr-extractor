//! Transactional command inbox and queued work behavior.

use extractor_eventing::{Reception, consume_capture};
use extractor_persistence::test_support::TestDatabase;
use serde_json::json;

const SUBJECT: &str = "cmd.content.capture.requested.v1";

#[tokio::test]
async fn one_command_creates_one_queued_run_under_redelivery()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let operation_id = uuid::Uuid::now_v7();
    let command = serde_json::to_vec(&json!({
        "command_id": uuid::Uuid::now_v7(),
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-21T10:00:00Z",
        "operation_id": operation_id,
        "tenant_id": format!("user:{}", uuid::Uuid::now_v7()),
        "correlation_id": format!("operation:{operation_id}"),
        "idempotency_key": "capture-queued-once",
        "payload": { "url": "https://example.test/article?utm_source=noise" }
    }))?;

    assert_eq!(
        consume_capture(database.database.pool(), SUBJECT, &command).await?,
        Reception::Applied
    );
    assert_eq!(
        consume_capture(database.database.pool(), SUBJECT, &command).await?,
        Reception::Duplicate
    );

    for (table, expected) in [
        ("extractor.inbox_events", 1_i64),
        ("extractor.sources", 1_i64),
        ("extractor.extraction_runs", 1_i64),
    ] {
        let count: i64 = sqlx::query_scalar(&format!("select count(*) from {table}"))
            .fetch_one(database.database.pool())
            .await?;
        assert_eq!(count, expected, "{table}");
    }
    let status: String = sqlx::query_scalar("select status from extractor.extraction_runs")
        .fetch_one(database.database.pool())
        .await?;
    assert_eq!(status, "queued");
    let has_document_id: bool =
        sqlx::query_scalar("select document_id is not null from extractor.extraction_runs")
            .fetch_one(database.database.pool())
            .await?;
    assert!(has_document_id);

    database.cleanup().await?;
    Ok(())
}
