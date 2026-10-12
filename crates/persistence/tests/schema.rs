//! Current extractor schema integration behavior.

use extractor_persistence::test_support::TestDatabase;
use sqlx::Row as _;

#[tokio::test]
async fn owned_schema_applies_once_with_all_item_six_tables()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    database.database.apply_schema().await?;

    let rows = sqlx::query(
        "select table_name from information_schema.tables
          where table_schema = 'extractor'
          order by table_name",
    )
    .fetch_all(database.database.pool())
    .await?;
    let tables = rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("table_name"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        tables,
        [
            "artifacts",
            "candidates",
            "extraction_runs",
            "fetches",
            "inbox_events",
            "media_archives",
            "outbox_events",
            "provider_resolutions",
            "render_budgets",
            "sources",
        ]
    );

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_blob_row_without_a_digest_violates_the_schema_check()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let urn = format!("urn:ratatoskr:blob:sha256:{}", "a".repeat(64));

    let blob_without_digest = insert_source(&database, &urn, "blob", None).await;
    let url_with_digest = insert_source(&database, &urn, "url", Some(&"a".repeat(64))).await;
    let blob_with_short_digest = insert_source(&database, &urn, "blob", Some("abc")).await;
    let valid_blob = insert_source(&database, &urn, "blob", Some(&"a".repeat(64))).await;

    database.cleanup().await?;
    assert!(
        blob_without_digest.is_err(),
        "a blob source needs its digest"
    );
    assert!(
        url_with_digest.is_err(),
        "a url source must not carry a digest"
    );
    assert!(
        blob_with_short_digest.is_err(),
        "the digest must be 64 lowercase hex"
    );
    assert!(
        valid_blob.is_ok(),
        "a complete blob source must be accepted"
    );
    Ok(())
}

async fn insert_source(
    database: &TestDatabase,
    address: &str,
    kind: &str,
    digest: Option<&str>,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query(
        "insert into extractor.sources
             (source_id, owner_id, original_url, normalized_url, canonical_url, host,
              classification, created_at, source_kind, blob_digest_hex)
         values ($1, $2, $3, $3, $3, 'ratatoskr-telegram', 'blob', now(), $4, $5)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(address)
    .bind(kind)
    .bind(digest)
    .execute(database.database.pool())
    .await
}

#[tokio::test]
async fn a_run_claim_is_all_three_columns_or_none() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let urn = format!("urn:ratatoskr:blob:sha256:{}", "b".repeat(64));
    let source = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into extractor.sources
             (source_id, owner_id, original_url, normalized_url, canonical_url, host,
              classification, created_at, source_kind, blob_digest_hex)
         values ($1, $2, $3, $3, $3, 'ratatoskr-telegram', 'blob', now(), 'blob', $4)",
    )
    .bind(source)
    .bind(uuid::Uuid::now_v7())
    .bind(&urn)
    .bind("b".repeat(64))
    .execute(database.database.pool())
    .await?;

    let none = insert_run(&database, source, (None, None, None)).await;
    let whole = insert_run(
        &database,
        source,
        (
            Some("ratatoskr-telegram"),
            Some("application/pdf"),
            Some(10),
        ),
    )
    .await;
    let owner_only = insert_run(&database, source, (Some("ratatoskr-telegram"), None, None)).await;
    let negative = insert_run(
        &database,
        source,
        (
            Some("ratatoskr-telegram"),
            Some("application/pdf"),
            Some(-1),
        ),
    )
    .await;
    let empty_owner = insert_run(
        &database,
        source,
        (Some(""), Some("application/pdf"), Some(10)),
    )
    .await;

    database.cleanup().await?;
    assert!(none.is_ok(), "a url run carries no claim");
    assert!(whole.is_ok(), "a complete claim must be accepted");
    assert!(owner_only.is_err(), "a claim is all three columns or none");
    assert!(negative.is_err(), "a length is never negative");
    assert!(
        empty_owner.is_err(),
        "the owner is bounded to 1..=64 characters"
    );
    Ok(())
}

async fn insert_run(
    database: &TestDatabase,
    source: uuid::Uuid,
    claim: (Option<&str>, Option<&str>, Option<i64>),
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    let command = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into extractor.inbox_events
             (command_id, subject, command_type, producer, received_at)
         values ($1, 'cmd.content.capture.requested.v1', 'content.capture.requested.v1',
                 'ratatoskr-platform', now())",
    )
    .bind(command)
    .execute(database.database.pool())
    .await?;
    sqlx::query(
        "insert into extractor.extraction_runs
             (run_id, command_id, operation_id, owner_id, correlation_id, source_id,
              status, policy_version, normalizer_version, parser_version, queued_at,
              blob_owner, blob_media_type, blob_length_bytes)
         values ($1, $2, $3, $4, 'operation:x', $5, 'queued', 'p', 'n', 'v', now(),
                 $6, $7, $8)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(command)
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(source)
    .bind(claim.0)
    .bind(claim.1)
    .bind(claim.2)
    .execute(database.database.pool())
    .await
}
