//! Lease, retry, and acknowledged `JetStream` publication.

use std::future::Future;

use extractor_eventing::{
    NatsPublisher, PublishError, Publisher, consume_capture, run_outbox_once,
};
use extractor_persistence::test_support::TestDatabase;
use serde_json::json;

struct RefusingPublisher;

impl Publisher for RefusingPublisher {
    fn publish(
        &self,
        _subject: &str,
        _payload: &[u8],
        _message_id: &str,
    ) -> impl Future<Output = Result<(), PublishError>> + Send {
        std::future::ready(Err(PublishError::new(std::io::Error::other(
            "injected refusal",
        ))))
    }
}

#[tokio::test]
async fn publisher_retries_without_marking_an_unacknowledged_message()
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
        "idempotency_key": "capture-outbox-retry",
        "payload": { "url": "https://example.test/article" }
    }))?;
    consume_capture(
        database.database.pool(),
        "cmd.content.capture.requested.v1",
        &command,
    )
    .await?;

    let diagnostic_rows: i64 = sqlx::query_scalar("select count(*) from extractor.outbox_events")
        .fetch_one(database.database.pool())
        .await?;
    eprintln!("DIAG-CI outbox rows after capture = {diagnostic_rows}");

    let failed = run_outbox_once(database.database.pool(), &RefusingPublisher, "test", 10).await?;
    assert_eq!(failed.claimed, 1);
    assert_eq!(failed.failed, 1);
    let retryable: bool = sqlx::query_scalar(
        "select attempts = 1 and published_at is null and next_attempt_at > now()
           from extractor.outbox_events",
    )
    .fetch_one(database.database.pool())
    .await?;
    assert!(retryable);

    sqlx::query("update extractor.outbox_events set next_attempt_at = now()")
        .execute(database.database.pool())
        .await?;
    let publisher = NatsPublisher::connect(&nats_url()).await?;
    publisher.ensure_event_stream().await?;
    let outcome = run_outbox_once(database.database.pool(), &publisher, "test", 10).await?;
    assert_eq!(outcome.published, 1);
    let settled: bool = sqlx::query_scalar(
        "select attempts = 1 and published_at is not null and claimed_until is null
           from extractor.outbox_events",
    )
    .fetch_one(database.database.pool())
    .await?;
    assert!(settled);
    assert_eq!(
        run_outbox_once(database.database.pool(), &publisher, "test", 10)
            .await?
            .claimed,
        0
    );

    database.cleanup().await?;
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
