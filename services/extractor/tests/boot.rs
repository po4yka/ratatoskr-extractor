//! Real process smoke test.

use std::net::{Ipv4Addr, SocketAddr};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use extractor_eventing::NatsPublisher;
use extractor_test_support::TemporaryBlobRoot;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn configured_process_serves_admin_only() -> Result<(), Box<dyn std::error::Error>> {
    let root = TemporaryBlobRoot::create().await?;
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;
    drop(listener);
    let binary = env!("CARGO_BIN_EXE_ratatoskr-extractor");
    let database_url = database_url();
    let bus_url = bus_url();
    let durable = format!("extractor_boot_{}", uuid::Uuid::now_v7().simple());
    let stream = prepare_durable(&bus_url, &durable).await?;

    let check = Command::new(binary)
        .arg("check-config")
        .env("RATATOSKR__BLOBS__ROOT", root.path())
        .env("RATATOSKR__DATABASE__URL", &database_url)
        .env("RATATOSKR__BUS__URL", &bus_url)
        .env("RATATOSKR__BUS__DURABLE_NAME", &durable)
        .env("RATATOSKR__ADMIN__BIND", address.to_string())
        .output()?;
    assert!(check.status.success());
    assert!(std::net::TcpStream::connect(address).is_err());

    let child = Command::new(binary)
        .env("RATATOSKR__BLOBS__ROOT", root.path())
        .env("RATATOSKR__DATABASE__URL", &database_url)
        .env("RATATOSKR__BUS__URL", &bus_url)
        .env("RATATOSKR__BUS__DURABLE_NAME", &durable)
        .env("RATATOSKR__ADMIN__BIND", address.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut process = ProcessGuard(child);
    wait_for_status(address, "/health/ready", 200).await?;

    for path in ["/health/live", "/health/ready", "/metrics", "/version"] {
        assert_eq!(http_status(address, path).await?, 200);
    }
    assert_eq!(http_status(address, "/fetch").await?, 404);

    let signal = Command::new("kill")
        .arg("-TERM")
        .arg(process.0.id().to_string())
        .status()?;
    assert!(signal.success());
    wait_for_exit(&mut process.0).await?;
    stream.delete_consumer(&durable).await?;
    Ok(())
}

async fn prepare_durable(
    bus_url: &str,
    durable: &str,
) -> Result<async_nats::jetstream::stream::Stream, Box<dyn std::error::Error>> {
    let publisher = NatsPublisher::connect(bus_url).await?;
    publisher.ensure_command_stream().await?;
    let stream = publisher.context().get_stream("ratatoskr_commands").await?;
    stream
        .get_or_create_consumer(
            durable,
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some(durable.to_owned()),
                filter_subject: "cmd.content.capture.requested.v1".to_owned(),
                deliver_policy: async_nats::jetstream::consumer::DeliverPolicy::New,
                ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                ack_wait: Duration::from_secs(30),
                max_deliver: 12,
                ..async_nats::jetstream::consumer::pull::Config::default()
            },
        )
        .await?;
    Ok(stream)
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only database location is not process configuration"
)]
fn database_url() -> String {
    match std::env::var("EXTRACTOR_TEST_DATABASE_URL") {
        Ok(value) => value,
        Err(_) => "postgres://extractor:extractor@127.0.0.1:5434/extractor".to_owned(),
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only broker location is not process configuration"
)]
fn bus_url() -> String {
    match std::env::var("EXTRACTOR_TEST_NATS_URL") {
        Ok(value) => value,
        Err(_) => "nats://127.0.0.1:4222".to_owned(),
    }
}

async fn wait_for_status(
    address: SocketAddr,
    path: &str,
    expected: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(status) = http_status(address, path).await
            && status == expected
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("admin listener did not become ready".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn http_status(address: SocketAddr, path: &str) -> Result<u16, Box<dyn std::error::Error>> {
    let mut stream = tokio::net::TcpStream::connect(address).await?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    let line = response
        .lines()
        .next()
        .ok_or("HTTP status line is missing")?;
    let status = line
        .split_ascii_whitespace()
        .nth(1)
        .ok_or("HTTP status code is missing")?
        .parse()?;
    Ok(status)
}

async fn wait_for_exit(child: &mut Child) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            return Err("extractor exited unsuccessfully".into());
        }
        if Instant::now() >= deadline {
            return Err("extractor did not stop after SIGTERM".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
