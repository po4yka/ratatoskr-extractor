//! Provider resolution steps commit atomically with the run's terminal facts.

use extractor_eventing::{Completion, ResolutionStep, claim_queued_run, consume_capture, fail_run};
use extractor_persistence::test_support::TestDatabase;
use serde_json::json;
use sqlx::Row as _;

const SUBJECT: &str = "cmd.content.capture.requested.v1";

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one hermetic scenario drives a full terminal transition and asserts every persisted row"
)]
async fn resolution_steps_commit_with_terminal_state() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let operation_id = uuid::Uuid::now_v7();
    let command = serde_json::to_vec(&json!({
        "command_id": uuid::Uuid::now_v7(),
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-21T10:00:00Z",
        "operation_id": operation_id,
        "tenant_id": format!("user:{}", uuid::Uuid::now_v7()),
        "correlation_id": format!("operation:{operation_id}"),
        "idempotency_key": "capture-resolution-steps-commit",
        "payload": { "url": "https://example.test/article" }
    }))?;
    consume_capture(database.database.pool(), SUBJECT, &command).await?;
    let claimed = claim_queued_run(database.database.pool(), "test-worker", 60).await?;
    let run_id = claimed.expect("queued run must exist").run_id;

    let steps = [
        ResolutionStep {
            ordinal: 0,
            kind: "provider_attempt",
            outcome: Some("unusable"),
            failure_class: Some("provider_schema_mismatch"),
            resolved_url: None,
        },
        ResolutionStep {
            ordinal: 1,
            kind: "html_fallback",
            outcome: Some("fallback_started"),
            failure_class: None,
            resolved_url: Some("https://example.test/article"),
        },
    ];

    assert_eq!(
        fail_run(
            database.database.pool(),
            run_id,
            "provider_unresolved",
            false,
            &steps
        )
        .await?,
        Completion::Applied
    );

    let run = sqlx::query(
        "select status, last_error_class
         from extractor.extraction_runs where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(database.database.pool())
    .await?;
    assert_eq!(run.try_get::<String, _>("status")?, "failed");
    assert_eq!(
        run.try_get::<Option<String>, _>("last_error_class")?
            .as_deref(),
        Some("provider_unresolved")
    );

    let stored = sqlx::query(
        "select ordinal, kind, outcome, failure_class, resolved_url
         from extractor.provider_resolutions where run_id = $1 order by ordinal",
    )
    .bind(run_id)
    .fetch_all(database.database.pool())
    .await?;
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].try_get::<i32, _>("ordinal")?, 0);
    assert_eq!(stored[0].try_get::<String, _>("kind")?, "provider_attempt");
    assert_eq!(
        stored[0]
            .try_get::<Option<String>, _>("outcome")?
            .as_deref(),
        Some("unusable")
    );
    assert_eq!(
        stored[0]
            .try_get::<Option<String>, _>("failure_class")?
            .as_deref(),
        Some("provider_schema_mismatch")
    );
    assert!(
        stored[0]
            .try_get::<Option<String>, _>("resolved_url")?
            .is_none()
    );
    assert_eq!(stored[1].try_get::<i32, _>("ordinal")?, 1);
    assert_eq!(stored[1].try_get::<String, _>("kind")?, "html_fallback");
    assert!(
        stored[1]
            .try_get::<Option<String>, _>("failure_class")?
            .is_none()
    );
    assert_eq!(
        stored[1]
            .try_get::<Option<String>, _>("resolved_url")?
            .as_deref(),
        Some("https://example.test/article")
    );

    sqlx::query(
        "insert into extractor.fetches
             (fetch_id, run_id, final_url, http_status, media_type,
              wire_bytes, decoded_bytes, attempts, cache_outcome, fetched_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(run_id)
    .bind("https://example.test/article")
    .bind(200_i32)
    .bind("text/html")
    .bind(17_i64)
    .bind(17_i64)
    .bind(1_i32)
    .bind("fresh")
    .execute(database.database.pool())
    .await?;

    let digest_hex = format!("{:064}", 0);
    sqlx::query(
        "insert into extractor.artifacts
             (artifact_id, run_id, kind, owner_service, digest_algorithm,
              digest_hex, media_type, length_bytes, created_at)
         values ($1, $2, 'raw_source', 'ratatoskr-extractor', 'sha256',
                 $3, 'text/html', $4, now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(run_id)
    .bind(digest_hex)
    .bind(17_i64)
    .execute(database.database.pool())
    .await?;

    database.cleanup().await?;
    Ok(())
}
