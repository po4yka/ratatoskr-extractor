//! Capture-command consumption against `PostgreSQL` 17.

use extractor_eventing::{Reception, claim_queued_run, consume_capture};
use extractor_persistence::test_support::TestDatabase;
use extractor_test_support::capture::CaptureCommandJson;
use ratatoskr_event_envelope::{EventEnvelope, EventPayload as _};
use ratatoskr_operation_contracts::{OperationReported, OperationStatus};
use serde_json::json;
use sqlx::Row as _;

const SUBJECT: &str = "cmd.content.capture.requested.v1";

#[tokio::test]
async fn a_consumed_capture_command_enqueues_one_operation_report()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let document = CaptureCommandJson::url("https://example.test/article");
    let operation_id = document.operation_id;
    let mut command = document.to_value();
    command["future_envelope_member"] = json!(true);

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
    let command = CaptureCommandJson::url("https://example.test/article")
        .with_idempotency("capture-redelivery")
        .to_bytes();

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

const PDF_DIGEST: &str = "33d13663b80c35b99fe73e7ee27db248affa1fe18c0b1d5b7237c98f75425726";

#[tokio::test]
async fn a_blob_capture_queues_a_blob_run() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let capture =
        CaptureCommandJson::blob("ratatoskr-telegram", PDF_DIGEST, "application/pdf", 13_264);
    let tenant = capture.tenant_user;

    consume_capture(database.database.pool(), SUBJECT, &capture.to_bytes()).await?;

    let source = sqlx::query(
        "select source_kind, original_url, normalized_url, canonical_url, host, classification,
                blob_owner, blob_digest_hex, blob_media_type, blob_length_bytes, owner_id
           from extractor.sources",
    )
    .fetch_all(database.database.pool())
    .await?;
    assert_eq!(source.len(), 1, "one blob capture must create one source");
    let source = source.first().ok_or("the source row is missing")?;
    let urn = format!("urn:ratatoskr:blob:sha256:{PDF_DIGEST}");
    assert_eq!(source.try_get::<String, _>("source_kind")?, "blob");
    for column in ["original_url", "normalized_url", "canonical_url"] {
        assert_eq!(source.try_get::<String, _>(column)?, urn, "{column}");
    }
    assert_eq!(source.try_get::<String, _>("host")?, "ratatoskr-telegram");
    assert_eq!(source.try_get::<String, _>("classification")?, "blob");
    assert_eq!(
        source.try_get::<String, _>("blob_owner")?,
        "ratatoskr-telegram"
    );
    assert_eq!(source.try_get::<String, _>("blob_digest_hex")?, PDF_DIGEST);
    assert_eq!(
        source.try_get::<String, _>("blob_media_type")?,
        "application/pdf"
    );
    assert_eq!(source.try_get::<i64, _>("blob_length_bytes")?, 13_264);
    assert_eq!(source.try_get::<uuid::Uuid, _>("owner_id")?, tenant);

    let versions: (String, String, String) = sqlx::query_as(
        "select status, parser_version, normalizer_version from extractor.extraction_runs",
    )
    .fetch_one(database.database.pool())
    .await?;
    assert_eq!(
        versions,
        (
            "queued".to_owned(),
            "pdf-v1".to_owned(),
            "blob-v1".to_owned()
        )
    );

    let run = claim_queued_run(database.database.pool(), "test-worker", 60)
        .await?
        .ok_or("the blob capture did not produce queued work")?;
    assert_eq!(run.url, urn);
    assert_eq!(run.classification, "blob");
    let blob = run
        .blob
        .ok_or("the claimed run must carry its blob reference")?;
    assert_eq!(blob.owner_service.as_str(), "ratatoskr-telegram");
    assert_eq!(blob.digest.hex.as_str(), PDF_DIGEST);
    assert_eq!(blob.media_type.as_str(), "application/pdf");
    assert_eq!(blob.length_bytes, 13_264);

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_second_identical_blob_by_the_same_owner_reuses_the_source()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let first =
        CaptureCommandJson::blob("ratatoskr-telegram", PDF_DIGEST, "application/pdf", 13_264);
    let mut second =
        CaptureCommandJson::blob("ratatoskr-telegram", PDF_DIGEST, "application/pdf", 13_264);
    second.tenant_user = first.tenant_user;
    let mut other_owner =
        CaptureCommandJson::blob("ratatoskr-telegram", PDF_DIGEST, "application/pdf", 13_264);
    other_owner.tenant_user = uuid::Uuid::now_v7();

    for command in [&first, &second, &other_owner] {
        consume_capture(database.database.pool(), SUBJECT, &command.to_bytes()).await?;
    }

    let sources: i64 = sqlx::query_scalar("select count(*) from extractor.sources")
        .fetch_one(database.database.pool())
        .await?;
    let runs: i64 = sqlx::query_scalar("select count(*) from extractor.extraction_runs")
        .fetch_one(database.database.pool())
        .await?;
    assert_eq!(
        sources, 2,
        "the same owner shares a source, another owner does not"
    );
    assert_eq!(runs, 3, "every command queues its own run");

    database.cleanup().await?;
    Ok(())
}
