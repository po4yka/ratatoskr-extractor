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
              classification, created_at, source_kind, blob_owner, blob_digest_hex,
              blob_media_type, blob_length_bytes)
         values ($1, $2, $3, $3, $3, 'ratatoskr-telegram', 'blob', now(), $4,
                 case when $5::text is null then null else 'ratatoskr-telegram' end, $5,
                 case when $5::text is null then null else 'application/pdf' end,
                 case when $5::text is null then null else 10::bigint end)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(address)
    .bind(kind)
    .bind(digest)
    .execute(database.database.pool())
    .await
}
