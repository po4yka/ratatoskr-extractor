//! Admin-plane lifecycle tests.

use std::net::{Ipv4Addr, SocketAddr};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, pull};
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use extractor_eventing::NatsPublisher;
use extractor_persistence::test_support::TestDatabase;
use extractor_service::{RuntimeHealth, admin_router};
use extractor_test_support::TemporaryBlobRoot;
use extractor_test_support::broker::{AuthorizedBroker, TestIdentity};
use sqlx::ConnectOptions as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tower::ServiceExt as _;

const IDENTITY: &str = include_str!("../../../deploy/nats/identity.conf");
const REPORT_SUBJECT_ALLOW: &str = "\"evt.platform.operation.reported.v1\",";

#[tokio::test]
async fn readiness_follows_startup_and_drain() -> Result<(), Box<dyn std::error::Error>> {
    let health = RuntimeHealth::new();
    let router = admin_router(health.clone(), || "fetch_total 0\n".to_owned());

    assert_status(&router, "/health/live", StatusCode::OK).await?;
    assert_status(&router, "/health/ready", StatusCode::SERVICE_UNAVAILABLE).await?;

    health.mark_ready();
    assert_status(&router, "/health/ready", StatusCode::OK).await?;

    health.begin_drain();
    assert_status(&router, "/health/live", StatusCode::OK).await?;
    assert_status(&router, "/health/ready", StatusCode::SERVICE_UNAVAILABLE).await?;
    assert_status(&router, "/metrics", StatusCode::OK).await?;
    assert_status(&router, "/version", StatusCode::OK).await?;
    Ok(())
}

async fn assert_status(
    router: &Router,
    path: &str,
    expected: StatusCode,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = router
        .clone()
        .oneshot(Request::get(path).body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), expected);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL),
        Some(&HeaderValue::from_static("no-store"))
    );
    Ok(())
}

struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A publisher the broker refuses must withdraw readiness, and an operator fixing the ACL must
/// restore it, without the relay ever stopping (XR-021 CONTRACTS.md R2-03).
#[tokio::test]
async fn readiness_follows_the_outbox_publisher() -> Result<(), Box<dyn std::error::Error>> {
    // The shipped EXTRACTOR stanza minus the operation-report subject: the broker accepts the
    // connection and the capture durable, and silently drops the publish.
    let identity = TestIdentity::generate()?;
    assert!(IDENTITY.contains(REPORT_SUBJECT_ALLOW));
    let broker = AuthorizedBroker::start(
        &identity.substitute_into(&IDENTITY.replace(REPORT_SUBJECT_ALLOW, ""))?,
    )
    .await?;
    let seed = identity.write_seed(broker.directory(), "extractor")?;
    provision_topology(&broker).await?;

    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let command_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into extractor.inbox_events (command_id, subject, command_type, producer, received_at)
         values ($1, 'cmd.content.capture.requested.v1', 'content.capture.requested.v1',
                 'ratatoskr-platform', now())",
    )
    .bind(command_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.outbox_events
             (outbox_id, message_id, causation_command_id, operation_id, subject, payload,
              enqueued_at, next_attempt_at)
         values ($1, $2, $3, $4, 'evt.platform.operation.reported.v1', '{\"probe\": true}'::jsonb,
                 now(), now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::now_v7())
    .bind(command_id)
    .bind(uuid::Uuid::now_v7())
    .execute(pool)
    .await?;

    let root = TemporaryBlobRoot::create().await?;
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;
    drop(listener);
    let database_url = pool.connect_options().to_url_lossy().to_string();
    let _process = ProcessGuard(
        Command::new(env!("CARGO_BIN_EXE_ratatoskr-extractor"))
            .env("RATATOSKR__BLOBS__ROOT", root.path())
            .env("RATATOSKR__DATABASE__URL", &database_url)
            .env("RATATOSKR__BUS__URL", broker.url())
            .env("RATATOSKR__BUS__NKEY_SEED_PATH", &seed)
            .env("RATATOSKR__BUS__POLL_INTERVAL_MS", "100")
            .env("RATATOSKR__ADMIN__BIND", address.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    wait_for_status(address, "/health/ready", 200, Duration::from_secs(10)).await?;

    // Three refused attempts of the queued row, each waiting out the acknowledgement deadline.
    wait_for_status(address, "/health/ready", 503, Duration::from_secs(60)).await?;
    let (_, metrics) = http_get(address, "/metrics").await?;
    assert!(
        metrics.contains("ratatoskr_extractor_outbox_publisher_unhealthy_total"),
        "the unhealthy verdict must be counted:\n{metrics}"
    );
    let published: bool =
        sqlx::query_scalar("select published_at is not null from extractor.outbox_events")
            .fetch_one(pool)
            .await?;
    assert!(!published);

    // The operator fixes the ACL; the relay, which never stopped, delivers on its next attempt.
    broker.reload_users(&identity.substitute_into(IDENTITY)?)?;
    wait_for_status(address, "/health/ready", 200, Duration::from_secs(60)).await?;
    let published: bool =
        sqlx::query_scalar("select published_at is not null from extractor.outbox_events")
            .fetch_one(pool)
            .await?;
    assert!(published);
    database.cleanup().await?;
    Ok(())
}

/// Provisions the streams and the two fixed durables the way Edge does.
async fn provision_topology(broker: &AuthorizedBroker) -> Result<(), Box<dyn std::error::Error>> {
    let administrator = NatsPublisher::connect_with_options(
        broker.url(),
        async_nats::ConnectOptions::with_user_and_password(
            AuthorizedBroker::ADMIN_USER.to_owned(),
            broker.admin_password().to_owned(),
        ),
    )
    .await?;
    administrator.ensure_command_stream().await?;
    administrator.ensure_event_stream().await?;
    let context = administrator.context();
    context
        .create_consumer_on_stream(
            pull::Config {
                durable_name: Some("ratatoskr_extractor_capture".to_owned()),
                filter_subject: "cmd.content.capture.requested.v1".to_owned(),
                ack_policy: AckPolicy::Explicit,
                ack_wait: Duration::from_secs(30),
                max_deliver: 12,
                ..pull::Config::default()
            },
            "ratatoskr_commands",
        )
        .await?;
    context
        .create_consumer_on_stream(
            pull::Config {
                durable_name: Some("ratatoskr_extractor_render_awaits".to_owned()),
                filter_subject: "evt.content.render.>".to_owned(),
                ack_policy: AckPolicy::None,
                deliver_policy: DeliverPolicy::New,
                ack_wait: Duration::from_secs(30),
                ..pull::Config::default()
            },
            "ratatoskr_events",
        )
        .await?;
    Ok(())
}

async fn wait_for_status(
    address: SocketAddr,
    path: &str,
    expected: u16,
    within: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + within;
    loop {
        if let Ok((status, _)) = http_get(address, path).await
            && status == expected
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("{path} never answered {expected}").into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn http_get(
    address: SocketAddr,
    path: &str,
) -> Result<(u16, String), Box<dyn std::error::Error>> {
    let mut stream = tokio::net::TcpStream::connect(address).await?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .ok_or("HTTP status is missing")?
        .parse()?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    Ok((status, body))
}
