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
            "sources",
        ]
    );

    database.cleanup().await?;
    Ok(())
}
