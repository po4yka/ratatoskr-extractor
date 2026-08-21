//! Capture-command consumption against `PostgreSQL` 17.

use extractor_eventing::{Reception, consume_capture};
use extractor_persistence::test_support::TestDatabase;
use ratatoskr_event_envelope::{EventEnvelope, EventPayload as _};
use ratatoskr_operation_contracts::{OperationReported, OperationStatus};
use serde_json::json;
use sqlx::Row as _;

const SUBJECT: &str = "cmd.content.capture.requested.v1";

#[tokio::test]
async fn a_consumed_capture_command_enqueues_one_operation_report()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let operation_id = uuid::Uuid::now_v7();
    let command = json!({
        "command_id": uuid::Uuid::now_v7(),
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-21T10:00:00Z",
        "operation_id": operation_id,
        "tenant_id": format!("user:{}", uuid::Uuid::now_v7()),
        "correlation_id": format!("operation:{operation_id}"),
        "idempotency_key": "capture-1",
        "payload": { "url": "https://example.test/article" },
        "future_envelope_member": true
    });

    consume_capture(
        database.database.pool(),
        SUBJECT,
        &serde_json::to_vec(&command)?,
    )
    .await?;

    let rows = sqlx::query("select subject, payload from extractor.outbox_events")
        .fetch_all(database.database.pool())
        .await?;
    assert_eq!(
        rows.len(),
        1,
        "one consumed command must enqueue one report"
    );
    let row = rows.first().ok_or("the report row is missing")?;
    let subject: String = row.try_get("subject")?;
    let payload: serde_json::Value = row.try_get("payload")?;
    assert_eq!(subject, "evt.platform.operation.reported.v1");
    let envelope: EventEnvelope = serde_json::from_value(payload)?;
    let report = envelope.payload_as::<OperationReported>()?;
    assert_eq!(envelope.event_type.to_wire(), OperationReported::EVENT_TYPE);
    assert_eq!(report.operation_id.to_string(), operation_id.to_string());
    assert_eq!(report.status, OperationStatus::Queued);

    let applied: bool = sqlx::query_scalar(
        "select applied_at is not null and outcome = 'applied'
           from extractor.inbox_events",
    )
    .fetch_one(database.database.pool())
    .await?;
    assert!(applied, "the inbox row must be marked applied");

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_redelivered_capture_command_remains_one_operation_report()
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
        "idempotency_key": "capture-redelivery",
        "payload": { "url": "https://example.test/article" }
    }))?;

    consume_capture(database.database.pool(), SUBJECT, &command).await?;
    let reception = consume_capture(database.database.pool(), SUBJECT, &command).await?;

    let count: i64 = sqlx::query_scalar("select count(*) from extractor.outbox_events")
        .fetch_one(database.database.pool())
        .await?;
    assert_eq!(count, 1, "redelivery must not enqueue a second report");
    assert_eq!(reception, Reception::Duplicate);

    database.cleanup().await?;
    Ok(())
}
