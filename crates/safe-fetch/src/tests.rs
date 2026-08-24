use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use extractor_blob_store::BlobStore;
use extractor_core::{ExtractorConfig, FetchConfig};
use extractor_test_support::{ScriptedResponse, ScriptedServer, TemporaryBlobRoot};
use futures_util::stream;

use super::{FetchRequest, SafeFetcher};

#[tokio::test]
async fn one_response_is_streamed_to_one_matching_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![ScriptedResponse::chunks([
        Bytes::from_static(b"first "),
        Bytes::from_static(b"second"),
    ])])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port()];
    let fetcher = SafeFetcher::new_for_test(config, BlobStore::new(root.path()))?;

    let result = fetcher
        .fetch_for_test(FetchRequest::new(&server.uri("/article")))
        .await?;

    assert_eq!(server.request_count(), 1);
    assert_eq!(result.status, 200);
    assert_eq!(result.final_url.as_str(), server.uri("/article"));
    assert_eq!(result.wire_bytes, 12);
    assert_eq!(result.decoded_bytes, 12);
    assert_eq!(result.artifact.length_bytes, 12);
    assert_eq!(
        tokio::fs::read(fetcher.store.resolve(&result.artifact)?).await?,
        b"first second"
    );
    Ok(())
}

#[tokio::test]
async fn redirect_target_is_revalidated_before_the_next_request()
-> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![ScriptedResponse::redirect(
        "http://127.0.0.2:80/internal",
    )])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port(), 80];
    let fetcher = SafeFetcher::new_for_test(config, BlobStore::new(root.path()))?;

    let result = fetcher
        .fetch_for_test(FetchRequest::new(&server.uri("/redirect")))
        .await;

    assert!(matches!(result, Err(super::SafeFetchError::AddressPolicy)));
    assert_eq!(server.request_count(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn redirects_and_retries_share_one_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let started = tokio::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(10);
    super::within_total_deadline(deadline, async {
        tokio::time::sleep(std::time::Duration::from_secs(9)).await;
        Ok::<_, super::SafeFetchError>(())
    })
    .await?;

    let result = super::within_total_deadline(deadline, async {
        std::future::pending::<()>().await;
        Ok::<_, super::SafeFetchError>(())
    })
    .await;

    assert!(
        matches!(result, Err(super::SafeFetchError::TimeoutTotal)),
        "got {result:?}"
    );
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(10));
    Ok(())
}

#[tokio::test]
async fn declared_and_actual_wire_limits_stop_before_commit()
-> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([Bytes::from(vec![b'x'; 100])]).with_header(
            http::header::CONTENT_LENGTH,
            http::HeaderValue::from_static("100"),
        ),
        ScriptedResponse::chunks([Bytes::from_static(b"streamed-over-limit")]),
    ])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port()];
    config.max_wire_bytes = 8;
    config.max_decoded_bytes = 16;
    let fetcher = SafeFetcher::new_for_test(config, BlobStore::new(root.path()))?;

    let declared = fetcher
        .fetch_for_test(FetchRequest::new(&server.uri("/declared")))
        .await;
    assert!(
        matches!(declared, Err(super::SafeFetchError::DeclaredBodyTooLarge)),
        "got {declared:?}"
    );

    let actual = fetcher
        .fetch_for_test(FetchRequest::new(&server.uri("/actual")))
        .await;
    assert!(matches!(
        actual,
        Err(super::SafeFetchError::WireBodyTooLarge)
    ));
    assert_eq!(server.request_count(), 2);
    assert!(!tokio::fs::try_exists(root.path().join("sha256")).await?);
    Ok(())
}

#[tokio::test]
async fn decoded_limit_stops_a_small_compressed_expansion() -> Result<(), Box<dyn std::error::Error>>
{
    let compressed = Bytes::from_static(&[
        31, 139, 8, 0, 0, 0, 0, 0, 2, 19, 115, 116, 28, 88, 0, 0, 222, 138, 24, 4, 128, 0, 0, 0,
    ]);
    let server = ScriptedServer::start(vec![ScriptedResponse::chunks([compressed]).with_header(
        http::header::CONTENT_ENCODING,
        http::HeaderValue::from_static("gzip"),
    )])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port()];
    config.max_wire_bytes = 32;
    config.max_decoded_bytes = 64;
    let fetcher = SafeFetcher::new_for_test(config, BlobStore::new(root.path()))?;

    let result = fetcher
        .fetch_for_test(FetchRequest::new(&server.uri("/compressed")))
        .await;

    assert!(
        matches!(result, Err(super::SafeFetchError::DecodedBodyTooLarge)),
        "got {result:?}"
    );
    assert!(!tokio::fs::try_exists(root.path().join("sha256")).await?);
    Ok(())
}

#[tokio::test]
async fn cache_validators_survive_without_sensitive_headers()
-> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([Bytes::from_static(b"<!doctype html><p>ok</p>")])
            .with_header(
                http::header::ETAG,
                http::HeaderValue::from_static("\"abc\""),
            )
            .with_header(
                http::header::LAST_MODIFIED,
                http::HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"),
            )
            .with_header(
                http::header::CACHE_CONTROL,
                http::HeaderValue::from_static("max-age=60"),
            )
            .with_header(
                http::header::SET_COOKIE,
                http::HeaderValue::from_static("session=secret"),
            )
            .with_header(
                http::header::WWW_AUTHENTICATE,
                http::HeaderValue::from_static("Basic realm=secret"),
            ),
    ])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port()];
    let fetcher = SafeFetcher::new_for_test(config, BlobStore::new(root.path()))?;

    let result = fetcher
        .fetch_for_test(FetchRequest::new(&server.uri("/page?token=do-not-log")))
        .await?;

    assert_eq!(result.metadata.etag.as_deref(), Some("\"abc\""));
    assert_eq!(
        result.metadata.last_modified.as_deref(),
        Some("Wed, 21 Oct 2015 07:28:00 GMT")
    );
    assert_eq!(result.metadata.cache_control.as_deref(), Some("max-age=60"));
    assert_eq!(result.metadata.effective_media_type, "text/html");
    assert!(result.metadata.total_duration > std::time::Duration::ZERO);
    assert!(result.metadata.normalized_url.as_str().contains("/page"));
    let evidence = format!("{:?}", result.metadata);
    assert!(!evidence.contains("session=secret"));
    assert!(!evidence.contains("realm=secret"));
    Ok(())
}

#[tokio::test]
async fn not_modified_without_verified_bytes_is_an_integrity_error()
-> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([]).with_status(http::StatusCode::NOT_MODIFIED),
    ])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let prior_artifact = store
        .store(
            "text/plain",
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"prior"))]),
        )
        .await?;
    tokio::fs::remove_file(store.resolve(&prior_artifact)?).await?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port()];
    let fetcher = SafeFetcher::new_for_test(config, store)?;
    let request = super::FetchRequest {
        url: server.uri("/cached"),
        prior: Some(super::CacheRecord {
            artifact: prior_artifact,
            etag: Some("\"prior\"".to_owned()),
            last_modified: None,
            cache_control: None,
        }),
    };

    let result = fetcher.fetch_for_test(request).await;

    assert!(matches!(result, Err(super::SafeFetchError::CacheIntegrity)));
    Ok(())
}

#[tokio::test]
async fn not_modified_reuses_one_verified_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([])
            .with_status(http::StatusCode::NOT_MODIFIED)
            .with_header(
                http::header::ETAG,
                http::HeaderValue::from_static("\"prior\""),
            ),
    ])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let prior_artifact = store
        .store(
            "text/plain",
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"prior"))]),
        )
        .await?;
    let prior_path = store.resolve(&prior_artifact)?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port()];
    let fetcher = SafeFetcher::new_for_test(config, store)?;
    let request = super::FetchRequest {
        url: server.uri("/cached"),
        prior: Some(super::CacheRecord {
            artifact: prior_artifact.clone(),
            etag: Some("\"prior\"".to_owned()),
            last_modified: Some("Wed, 21 Oct 2015 07:28:00 GMT".to_owned()),
            cache_control: Some("max-age=60".to_owned()),
        }),
    };

    let result = fetcher.fetch_for_test(request).await?;

    assert_eq!(
        result.metadata.cache_outcome,
        super::CacheOutcome::Revalidated
    );
    assert_eq!(result.artifact, prior_artifact);
    assert_eq!(tokio::fs::read(prior_path).await?, b"prior");
    let requests = server.requests().await;
    let request = requests.first().ok_or("missing request")?;
    assert_eq!(
        request.headers.get(http::header::IF_NONE_MATCH),
        Some(&http::HeaderValue::from_static("\"prior\""))
    );
    assert_eq!(server.request_count(), 1);
    Ok(())
}

#[test]
fn policy_and_deterministic_failures_are_attempted_once() {
    let deterministic = [
        super::SafeFetchError::AddressPolicy,
        super::SafeFetchError::UnsupportedEncoding,
        super::SafeFetchError::DecodedBodyTooLarge,
        super::SafeFetchError::CacheIntegrity,
        super::SafeFetchError::RedirectLoop,
    ];

    assert!(
        deterministic
            .iter()
            .all(|error| !super::retry_eligible(error))
    );
    assert!(super::retry_eligible(&super::SafeFetchError::Transport));
}

#[test]
fn retry_after_http_date_uses_the_remaining_delay() {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::RETRY_AFTER,
        http::HeaderValue::from_static("Thu, 01 Jan 1970 00:00:10 GMT"),
    );

    assert_eq!(
        super::retry_after_at(&headers, std::time::UNIX_EPOCH),
        Some(std::time::Duration::from_secs(10))
    );
}

#[tokio::test]
async fn eligible_get_retry_honors_retry_after_jitter_and_deadline()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        super::retry_delay(0, 7, Some(std::time::Duration::from_secs(1))),
        std::time::Duration::from_secs(1)
    );
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([])
            .with_status(http::StatusCode::SERVICE_UNAVAILABLE)
            .with_header(
                http::header::RETRY_AFTER,
                http::HeaderValue::from_static("0"),
            ),
        ScriptedResponse::chunks([Bytes::from_static(b"recovered")]),
    ])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port()];
    config.max_retries = 1;
    config.total_timeout_ms = 5_000;
    let fetcher = SafeFetcher::new_for_test(config, BlobStore::new(root.path()))?;
    let started = tokio::time::Instant::now();

    let result = fetcher
        .fetch_for_test(FetchRequest::new(&server.uri("/transient")))
        .await?;

    assert_eq!(result.metadata.attempts, 2);
    assert_eq!(server.request_count(), 2);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(result.status, 200);
    Ok(())
}

#[tokio::test]
async fn per_host_capacity_refuses_before_dns() -> Result<(), Box<dyn std::error::Error>> {
    let server = ScriptedServer::start(vec![ScriptedResponse::chunks([Bytes::from_static(
        b"must-not-be-requested",
    )])])
    .await?;
    let root = TemporaryBlobRoot::create().await?;
    let mut config = ExtractorConfig::built_in(root.path()).fetch;
    config.allowed_ports = vec![server.port()];
    config.global_concurrency = 2;
    config.per_host_concurrency = 1;
    let fetcher = SafeFetcher::new_for_test(config, BlobStore::new(root.path()))?;
    let _occupied = fetcher.admission.try_enter("127.0.0.1")?;

    let result = fetcher
        .fetch_for_test(FetchRequest::new(&server.uri("/overloaded")))
        .await;

    assert!(matches!(result, Err(super::SafeFetchError::Overloaded)));
    assert_eq!(server.request_count(), 0);
    Ok(())
}

fn spawn_plain_server() -> (SocketAddr, Arc<Mutex<Vec<Instant>>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback listener");
    let addr = listener.local_addr().expect("local addr");
    let arrivals: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let log = Arc::clone(&arrivals);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                let mut buf = [0_u8; 2048];
                let _ = stream.read(&mut buf);
                log.lock().expect("arrival log").push(Instant::now());
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\n\r\nok",
                );
            });
        }
    });
    (addr, arrivals)
}

fn paced_fetcher(port: u16, interval_ms: u64, total_timeout_ms: u64) -> SafeFetcher {
    let root = std::env::temp_dir().join(format!("safe-fetch-pacing-{port}"));
    std::fs::create_dir_all(&root).expect("temp blob root");
    let config = FetchConfig {
        max_url_length: 2048,
        allowed_ports: vec![port],
        connect_timeout_ms: 1_000,
        first_byte_timeout_ms: 1_000,
        read_idle_timeout_ms: 1_000,
        total_timeout_ms,
        max_wire_bytes: 1 << 20,
        max_decoded_bytes: 1 << 20,
        max_redirects: 0,
        max_retries: 0,
        global_concurrency: 4,
        per_host_concurrency: 4,
        per_host_min_interval_ms: interval_ms,
    };
    SafeFetcher::new_for_test(config, BlobStore::new(&root)).expect("test fetcher")
}

#[tokio::test]
async fn requests_to_same_host_are_spaced_by_min_interval() {
    let (addr, arrivals) = spawn_plain_server();
    let fetcher = paced_fetcher(addr.port(), 60, 10_000);
    let url = format!("http://{addr}/paced");

    for _ in 0..3 {
        let result = fetcher.fetch_for_test(FetchRequest::new(&url)).await;
        assert!(result.is_ok(), "every paced request succeeds");
    }

    let times = arrivals.lock().expect("arrival log").clone();
    assert_eq!(times.len(), 3);
    for pair in times.windows(2) {
        let gap = pair[1].saturating_duration_since(pair[0]);
        assert!(
            gap >= Duration::from_millis(50),
            "inter-request gap {gap:?} collapsed below the configured spacing"
        );
    }
}

#[tokio::test]
async fn pacing_never_extends_operation_deadline() {
    let (addr, _arrivals) = spawn_plain_server();
    let fetcher = paced_fetcher(addr.port(), 500, 150);
    let url = format!("http://{addr}/deadline");

    let first = fetcher.fetch_for_test(FetchRequest::new(&url)).await;
    assert!(first.is_ok(), "the first request fits inside the deadline");

    let started = Instant::now();
    let second = fetcher.fetch_for_test(FetchRequest::new(&url)).await;
    let elapsed = started.elapsed();

    assert!(
        matches!(second, Err(super::SafeFetchError::TimeoutTotal)),
        "a wait beyond the remaining deadline surfaces the existing deadline class"
    );
    assert!(
        elapsed < Duration::from_millis(400),
        "pacing waited past the operation budget ({elapsed:?})"
    );
}
