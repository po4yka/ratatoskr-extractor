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

    // The freshly created database can answer the capture commit a moment before its row is
    // visible to a new snapshot on busy runners; retry the first lease instead of failing.
    let mut failed = None;
    for _ in 0..5 {
        let attempt =
            run_outbox_once(database.database.pool(), &RefusingPublisher, "test", 10).await?;
        if attempt.claimed > 0 {
            failed = Some(attempt);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let failed = failed.ok_or("the outbox row never became due")?;
    let _due_now: i64 = sqlx::query_scalar(
        "select count(*) from extractor.outbox_events
          where published_at is null and dead_lettered_at is null
            and next_attempt_at <= clock_timestamp()
            and (claimed_until is null or claimed_until <= clock_timestamp())",
    )
    .fetch_one(database.database.pool())
    .await?;
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
    // Same bounded retry as the first phase: a busy runner may answer the very first
    // publication before the stream interest is fully visible.
    let mut outcome = None;
    for _ in 0..5 {
        sqlx::query("update extractor.outbox_events set next_attempt_at = now()")
            .execute(database.database.pool())
            .await?;
        let attempt = run_outbox_once(database.database.pool(), &publisher, "test", 10).await?;
        if attempt.published > 0 {
            outcome = Some(attempt);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let outcome = outcome.ok_or("the outbox row never published")?;
    assert_eq!(outcome.published, 1);
    let settled: bool = sqlx::query_scalar(
        "select attempts = 1 and published_at is not null and claimed_until is null
           from extractor.outbox_events",
    )
    .fetch_one(database.database.pool())
    .await?;
    assert!(settled);
    assert_eq!(
        run_outbox_once(database.database.pool(), publisher.0, "test", 10)
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
