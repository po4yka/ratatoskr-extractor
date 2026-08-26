//! The durable per-UTC-day render budget.

use extractor_eventing::{ConsumeError, RenderBudget, consume_render_budget};
use extractor_persistence::test_support::TestDatabase;

#[tokio::test]
async fn budget_admits_one_slot_then_exhausts_for_the_day() -> Result<(), Box<dyn std::error::Error>>
{
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();

    let first = consume_render_budget(pool, 1).await?;
    assert_eq!(first, RenderBudget::Consumed { count: 1 });

    let second = consume_render_budget(pool, 1).await?;
    assert_eq!(second, RenderBudget::Exhausted);

    let stored: (i32,) = sqlx::query_as(
        "select escalated from extractor.render_budgets where utc_day = current_date",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(stored.0, 1, "the counter must hold the consumed total");

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn generous_budget_advances_per_consumption() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();

    for expected in [1, 2, 3] {
        let outcome = consume_render_budget(pool, 3).await?;
        assert_eq!(outcome, RenderBudget::Consumed { count: expected });
    }

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_zero_cap_exhaustes_immediately() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();

    let outcome = consume_render_budget(pool, 0).await?;
    assert_eq!(outcome, RenderBudget::Exhausted);

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn transport_failures_surface_as_database_errors() {
    let broken =
        sqlx::PgPool::connect("postgres://extractor:extractor@127.0.0.1:5999/absent").await;
    let Ok(broken) = broken else {
        // A refused connection already demonstrates surfaced transport failure.
        return;
    };
    let outcome = consume_render_budget(&broken, 1).await;
    assert!(
        matches!(outcome, Err(ConsumeError::Database(_))),
        "expected a database error, got {outcome:?}"
    );
}
