//! Lease, retry, and acknowledged `JetStream` publication.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};

use extractor_eventing::{
    NatsPublisher, OutboxHealth, PublishError, Publisher, consume_capture, run_outbox_once,
};
use extractor_persistence::test_support::TestDatabase;
use extractor_test_support::broker::AuthorizedBroker;
use extractor_test_support::capture::CaptureCommandJson;
use sqlx::PgPool;

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

/// Accepts every row.
struct AcceptingPublisher;

impl Publisher for AcceptingPublisher {
    fn publish(
        &self,
        _subject: &str,
        _payload: &[u8],
        _message_id: &str,
    ) -> impl Future<Output = Result<(), PublishError>> + Send {
        std::future::ready(Ok(()))
    }
}

/// Classes its first call as larger than the broker accepts and accepts every later call.
struct OversizeFirst {
    calls: AtomicUsize,
}

impl Publisher for OversizeFirst {
    fn publish(
        &self,
        _subject: &str,
        _payload: &[u8],
        _message_id: &str,
    ) -> impl Future<Output = Result<(), PublishError>> + Send {
        // Ordering: the counter orders nothing else; the single test task reads it after the await.
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        std::future::ready(if call == 0 {
            Err(PublishError::too_large())
        } else {
            Ok(())
        })
    }
}

/// Queues one outbox row per capture command and returns once all of them exist.
async fn queue_rows(
    pool: &PgPool,
    indexes: std::ops::Range<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    for index in indexes {
        let command = CaptureCommandJson::url(&format!("https://example.test/article-{index}"))
            .with_idempotency(&format!("capture-outbox-class-{index}"))
            .to_bytes();
        consume_capture(pool, "cmd.content.capture.requested.v1", &command).await?;
    }
    Ok(())
}

/// Makes every unpublished row due now, then runs one pass.
async fn due_pass<P: Publisher>(
    pool: &PgPool,
    publisher: &P,
) -> Result<extractor_eventing::OutboxReport, Box<dyn std::error::Error>> {
    sqlx::query(
        "update extractor.outbox_events set next_attempt_at = clock_timestamp()
          where published_at is null and dead_lettered_at is null",
    )
    .execute(pool)
    .await?;
    Ok(run_outbox_once(pool, publisher, "test", 10).await?)
}

#[tokio::test]
async fn transient_failures_never_dead_letter() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    queue_rows(pool, 0..1).await?;

    for pass in 1..=30 {
        let report = due_pass(pool, &RefusingPublisher).await?;
        assert_eq!(
            (report.claimed, report.failed, report.dead_lettered),
            (1, 1, 0),
            "pass {pass}"
        );
    }
    let (attempts, published, dead_lettered): (i32, bool, bool) = sqlx::query_as(
        "select attempts, published_at is not null, dead_lettered_at is not null
           from extractor.outbox_events",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!((attempts, published, dead_lettered), (30, false, false));

    let report = due_pass(pool, &AcceptingPublisher).await?;
    assert_eq!((report.published, report.failed), (1, 0));
    let published: bool =
        sqlx::query_scalar("select published_at is not null from extractor.outbox_events")
            .fetch_one(pool)
            .await?;
    assert!(published);
    database.cleanup().await?;
    Ok(())
}

/// One pass followed by the health observation of that pass.
async fn observed_pass<P: Publisher>(
    pool: &PgPool,
    health: &mut OutboxHealth,
    publisher: &P,
) -> Result<bool, Box<dyn std::error::Error>> {
    let report = due_pass(pool, publisher).await?;
    Ok(health.observe(pool, &report).await?)
}

#[tokio::test]
async fn repeated_refusals_mark_the_publisher_unhealthy_until_a_clean_pass()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let mut health = OutboxHealth::new();
    assert!(health.is_healthy());

    // An empty pass finds nothing to publish.
    let report = run_outbox_once(pool, &RefusingPublisher, "test", 10).await?;
    assert!(health.observe(pool, &report).await?);

    queue_rows(pool, 0..1).await?;
    // One and two refused attempts are a hiccup; the third is a stuck publisher.
    assert!(observed_pass(pool, &mut health, &RefusingPublisher).await?);
    assert!(observed_pass(pool, &mut health, &RefusingPublisher).await?);
    assert!(!observed_pass(pool, &mut health, &RefusingPublisher).await?);
    assert!(!health.is_healthy());

    // The row is in backoff, so the next poll finds nothing due. That says nothing about the
    // publisher, so the verdict must not flap back to healthy between attempts.
    let idle = run_outbox_once(pool, &RefusingPublisher, "test", 10).await?;
    assert_eq!(idle.claimed, 0);
    assert!(!health.observe(pool, &idle).await?);

    // A pass that publishes every due row clears it.
    assert!(observed_pass(pool, &mut health, &AcceptingPublisher).await?);
    assert!(health.is_healthy());

    // Nothing unpublished at all is healthy as well.
    let report = run_outbox_once(pool, &RefusingPublisher, "test", 10).await?;
    assert!(health.observe(pool, &report).await?);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn an_old_head_row_makes_a_first_refusal_unhealthy_and_undeliverable_rows_never_do()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let mut health = OutboxHealth::new();
    queue_rows(pool, 0..1).await?;
    sqlx::query("update extractor.outbox_events set enqueued_at = now() - interval '301 seconds'")
        .execute(pool)
        .await?;
    assert!(!observed_pass(pool, &mut health, &RefusingPublisher).await?);
    assert!(observed_pass(pool, &mut health, &AcceptingPublisher).await?);

    // A row classed undeliverable is out of the retry set, so it cannot hold the verdict down,
    // however many attempts it consumed.
    queue_rows(pool, 1..2).await?;
    sqlx::query("update extractor.outbox_events set attempts = 5 where published_at is null")
        .execute(pool)
        .await?;
    let publisher = OversizeFirst {
        calls: AtomicUsize::new(0),
    };
    let report = due_pass(pool, &publisher).await?;
    assert_eq!(report.dead_lettered, 1);
    assert!(health.observe(pool, &report).await?);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn an_oversize_row_is_undeliverable_at_once_and_does_not_block_later_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    queue_rows(pool, 0..2).await?;

    let publisher = OversizeFirst {
        calls: AtomicUsize::new(0),
    };
    let report = due_pass(pool, &publisher).await?;
    assert_eq!(
        (
            report.claimed,
            report.published,
            report.failed,
            report.dead_lettered
        ),
        (2, 1, 0, 1)
    );
    let rows: Vec<(i32, bool, bool, Option<String>)> = sqlx::query_as(
        "select attempts, published_at is not null, dead_lettered_at is not null, last_error
           from extractor.outbox_events order by enqueued_at, outbox_id",
    )
    .fetch_all(pool)
    .await?;
    assert_eq!(
        rows,
        vec![
            (1, false, true, Some("payload_too_large".to_owned())),
            (0, true, false, None),
        ]
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_body_larger_than_the_server_limit_is_classed_before_publishing()
-> Result<(), Box<dyn std::error::Error>> {
    let broker = AuthorizedBroker::start_with("", "max_payload: 65536").await?;
    let publisher = NatsPublisher::connect_with_options(
        broker.url(),
        async_nats::ConnectOptions::with_user_and_password(
            AuthorizedBroker::ADMIN_USER.to_owned(),
            broker.admin_password().to_owned(),
        ),
    )
    .await?;
    publisher.ensure_event_stream().await?;
    let subject = "evt.content.document.extracted.v1";

    // Above the server limit, and inside the limit but within the 1 KiB headroom the publisher
    // keeps for headers: both are classed permanent without waiting for an acknowledgement.
    for length in [128 * 1_024, 65_000] {
        let body = vec![b'x'; length];
        let started = std::time::Instant::now();
        let error = publisher
            .publish(subject, &body, &uuid::Uuid::now_v7().to_string())
            .await
            .expect_err("an oversize body must be refused");
        assert!(error.is_permanent(), "{length} bytes: {error}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{length} bytes waited for the broker"
        );
    }
    // A body under the limit still publishes.
    publisher
        .publish(
            subject,
            &vec![b'x'; 32 * 1_024],
            &uuid::Uuid::now_v7().to_string(),
        )
        .await?;
    let mut stream = publisher.context().get_stream("ratatoskr_events").await?;
    assert_eq!(stream.info().await?.state.messages, 1);
    Ok(())
}

#[tokio::test]
async fn publisher_retries_without_marking_an_unacknowledged_message()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let command = CaptureCommandJson::url("https://example.test/article")
        .with_idempotency("capture-outbox-retry")
        .to_bytes();
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
