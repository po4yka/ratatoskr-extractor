//! Provider post-resolution: classified runs resolve public provider APIs to a canonical
//! target before the ordinary retrieval path runs, recording every step.

use extractor_blob_store::BlobStore;
use extractor_core::ExtractorConfig;
use extractor_eventing::claim_queued_run;
use extractor_persistence::test_support::TestDatabase;
use extractor_safe_fetch::SafeFetcher;
use extractor_test_support::{ScriptedResponse, ScriptedServer, TemporaryBlobRoot};
use ratatoskr_identifiers::{BlobRef, DocumentId};
use serde_json::json;

const ARTICLE_MARKER: &str = "resolved_article_body_fixture_marker";
const DISCUSSION_MARKER_A: &str = "discussion_comment_fixture_marker_one";
const DISCUSSION_MARKER_B: &str = "discussion_comment_fixture_marker_two";
const ASK_MARKER: &str = "ask_hn_self_text_fixture_marker";
const REDDIT_SELF_MARKER: &str = "reddit_self_text_fixture_marker";
const CHALLENGE_MARKER: &str = "provider_challenge_page_fixture_marker";
const GENERIC_FALLBACK_MARKER: &str = "generic_fallback_article_fixture_marker";

#[path = "provider_resolution/link_post.rs"]
mod link_post;

fn fixture_comment(marker: &str) -> String {
    format!(
        "<p>The {marker} comment contributes steady discussion prose for the fixture thread, \
         giving the shared evaluator enough paragraph volume to accept the converted item \
         while keeping every byte of the payload deterministic across repeated runs.</p>"
    )
}

/// Mirrors the production inbox path closely enough to drive one claimed run directly.
async fn queue_direct(
    pool: &sqlx::PgPool,
    url: &str,
    classification: &str,
) -> Result<(), sqlx::Error> {
    let command_id = uuid::Uuid::now_v7();
    let operation_id = uuid::Uuid::now_v7();
    let owner_id = uuid::Uuid::now_v7();
    let source_id = uuid::Uuid::now_v7();
    let run_id = uuid::Uuid::now_v7();
    let document_id = DocumentId::new_v7().0;
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', ':']).next())
        .unwrap_or("localhost")
        .to_owned();
    sqlx::query(
        "insert into extractor.inbox_events (command_id, subject, command_type, producer, received_at)
         values ($1, 'cmd.content.capture.requested.v1', 'content.capture.requested.v1',
                 'ratatoskr-platform', transaction_timestamp())",
    )
    .bind(command_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.sources
             (source_id, owner_id, original_url, normalized_url, canonical_url, host,
              classification, created_at)
         values ($1, $2, $3, $3, $3, $4, $5, transaction_timestamp())",
    )
    .bind(source_id)
    .bind(owner_id)
    .bind(url)
    .bind(host)
    .bind(classification)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.extraction_runs
             (run_id, command_id, operation_id, owner_id, correlation_id, source_id, document_id,
              status, policy_version, normalizer_version, parser_version, queued_at)
         values ($1, $2, $3, $4, $5, $6, $7, 'queued', 'ssrf-v1', 'url-v1', 'html-v1',
                 transaction_timestamp())",
    )
    .bind(run_id)
    .bind(command_id)
    .bind(operation_id)
    .bind(owner_id)
    .bind(format!("operation:{operation_id}"))
    .bind(source_id)
    .bind(document_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn stored_document_contains(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    run_id: uuid::Uuid,
    needle: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    let row: (String, String, String, String, i64) = sqlx::query_as(
        "select owner_service, digest_algorithm, digest_hex, media_type, length_bytes
           from extractor.artifacts where run_id = $1 and kind = 'document_ir'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let reference: BlobRef = serde_json::from_value(json!({
        "owner_service": row.0,
        "digest": {"algorithm": row.1, "hex": row.2},
        "media_type": row.3,
        "length_bytes": u64::try_from(row.4)?,
    }))?;
    let path = store.verify(&reference).await?;
    let document_text = tokio::fs::read_to_string(path).await?;
    Ok(document_text.contains(needle))
}

#[tokio::test]
async fn hn_ask_post_completes_without_second_request() -> Result<(), Box<dyn std::error::Error>> {
    let payload = json!({
        "id": 2000,
        "title": "Ask fixture",
        "text": format!(
            "<p>{ASK_MARKER}</p>\
             <p>The ask body carries steady paragraph volume so the shared evaluator accepts \
             the converted story while every byte stays deterministic across repeated runs.</p>"
        ),
        "children": [
            {"id": 2001, "text": fixture_comment(DISCUSSION_MARKER_A), "children": []},
        ],
    });
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(serde_json::to_vec(&payload)?)]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    let address = server
        .uri("/api/v1/items/2000")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        "https://news.ycombinator.com/item?id=2000",
        "hacker_news",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        &config.parser,
        extractor_providers::SourceRoute::HackerNews,
        &address,
        &run,
    )
    .await?;

    let outcome: (String, Option<String>) = sqlx::query_as(
        "select status, last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(outcome.0, "succeeded");
    assert_eq!(outcome.1, None);
    let (fetch_total,): (i64,) =
        sqlx::query_as("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(
        fetch_total, 1,
        "the ask post never resolves a second target"
    );
    let (steps,): (i64,) =
        sqlx::query_as("select count(*) from extractor.provider_resolutions where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(steps, 0, "self-contained posts record no resolution steps");
    assert!(
        stored_document_contains(pool, &store, run.run_id, ASK_MARKER).await?,
        "the self-text body becomes the document"
    );
    assert_eq!(server.request_count(), 1);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn reddit_self_post_completes_without_second_request()
-> Result<(), Box<dyn std::error::Error>> {
    let payload = json!([
        {"kind": "Listing", "data": {"children": [
            {"kind": "t3", "data": {
                "title": "Self post fixture",
                "selftext": format!(
                    "<p>{REDDIT_SELF_MARKER}</p>\
                     <p>The self-text carries steady paragraph volume so the shared evaluator \
                     accepts the converted post while every byte stays deterministic.</p>"
                ),
            }},
            {"kind": "t1", "data": {"body": fixture_comment(DISCUSSION_MARKER_A)}},
        ]}},
    ]);
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(serde_json::to_vec(&payload)?)]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    let address = server
        .uri("/r/fixturerust/comments/self00/self_post_fixture.json")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        "https://www.reddit.com/r/fixturerust/comments/self00/self_post_fixture/",
        "reddit",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        &config.parser,
        extractor_providers::SourceRoute::Reddit,
        &address,
        &run,
    )
    .await?;

    let outcome: (String, Option<String>) = sqlx::query_as(
        "select status, last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(outcome.0, "succeeded");
    assert_eq!(outcome.1, None);
    let (fetch_total,): (i64,) =
        sqlx::query_as("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(
        fetch_total, 1,
        "the self post never resolves a second target"
    );
    let (steps,): (i64,) =
        sqlx::query_as("select count(*) from extractor.provider_resolutions where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(steps, 0, "self-contained posts record no resolution steps");
    assert!(
        stored_document_contains(pool, &store, run.run_id, REDDIT_SELF_MARKER).await?,
        "the self-text body becomes the document"
    );
    assert_eq!(server.request_count(), 1);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn transport_failure_still_terminates_run() -> Result<(), Box<dyn std::error::Error>> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![port];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    let address = format!("http://localhost:{port}/api/v1/items/3000");
    queue_direct(
        pool,
        "https://news.ycombinator.com/item?id=3000",
        "hacker_news",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        &config.parser,
        extractor_providers::SourceRoute::HackerNews,
        &address,
        &run,
    )
    .await?;

    let outcome: (String, Option<String>) = sqlx::query_as(
        "select status, last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(outcome.0, "failed");
    assert_eq!(
        outcome.1.as_deref(),
        Some("fetch"),
        "connection refusal keeps today's typed transport failure class"
    );
    let (fetch_total,): (i64,) =
        sqlx::query_as("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(
        fetch_total, 0,
        "no fetch row survives a failed provider request"
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one hermetic scenario scripts two responses and asserts both recorded outcomes"
)]
async fn non_json_provider_response_falls_through_to_html() -> Result<(), Box<dyn std::error::Error>>
{
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(format!(
            "<!doctype html><html><head><title>Access denied</title></head>\
             <body><p>{CHALLENGE_MARKER}</p></body></html>"
        ))])
        .with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
        ScriptedResponse::chunks([bytes::Bytes::from(format!(
            "<!doctype html><html><head><title>Original page</title></head>\
             <body><article><p>{GENERIC_FALLBACK_MARKER}</p>\
             <p>The original page carries steady paragraph volume so the shared evaluator \
             accepts the generic extraction while every byte stays deterministic.</p>\
             </article></body></html>"
        ))])
        .with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    let address = server
        .uri("/api/v1/items/4000")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        &server.uri("/original").replace("127.0.0.1", "localhost"),
        "hacker_news",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        &config.parser,
        extractor_providers::SourceRoute::HackerNews,
        &address,
        &run,
    )
    .await?;

    let outcome: (String, Option<String>) = sqlx::query_as(
        "select status, last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        outcome.0, "succeeded",
        "the generic attempt completes the run"
    );
    let kinds: Vec<(String, Option<String>)> = sqlx::query_as(
        "select kind, failure_class from extractor.provider_resolutions
          where run_id = $1 order by ordinal",
    )
    .bind(run.run_id)
    .fetch_all(pool)
    .await?;
    assert_eq!(
        kinds,
        vec![
            (
                "provider_attempt".to_string(),
                Some("provider_response".to_string())
            ),
            ("html_fallback".to_string(), None),
        ],
        "both the provider failure and the successful fallback are recorded"
    );
    let (fetch_total,): (i64,) =
        sqlx::query_as("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(fetch_total, 2);
    assert!(
        stored_document_contains(pool, &store, run.run_id, GENERIC_FALLBACK_MARKER).await?,
        "the document comes from the original page"
    );
    assert_eq!(server.request_count(), 2);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one hermetic scenario scripts two responses and asserts both recorded outcomes"
)]
async fn malformed_provider_schema_falls_through_instead_of_dying()
-> Result<(), Box<dyn std::error::Error>> {
    let broken_payload = json!({
        "title": "Broken schema fixture",
        "children": [],
    });
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(serde_json::to_vec(&broken_payload)?)])
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            ),
        ScriptedResponse::chunks([bytes::Bytes::from(format!(
            "<!doctype html><html><head><title>Original page</title></head>\
             <body><article><p>{GENERIC_FALLBACK_MARKER}</p>\
             <p>The original page carries steady paragraph volume so the shared evaluator \
             accepts the generic extraction while every byte stays deterministic.</p>\
             </article></body></html>"
        ))])
        .with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    let address = server
        .uri("/api/v1/items/5000")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        &server.uri("/original").replace("127.0.0.1", "localhost"),
        "hacker_news",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        &config.parser,
        extractor_providers::SourceRoute::HackerNews,
        &address,
        &run,
    )
    .await?;

    let outcome: (String, Option<String>) = sqlx::query_as(
        "select status, last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        outcome.0, "succeeded",
        "the schema violation degrades instead of dying"
    );
    let kinds: Vec<(String, Option<String>)> = sqlx::query_as(
        "select kind, failure_class from extractor.provider_resolutions
          where run_id = $1 order by ordinal",
    )
    .bind(run.run_id)
    .fetch_all(pool)
    .await?;
    assert_eq!(
        kinds,
        vec![
            (
                "provider_attempt".to_string(),
                Some("provider_schema".to_string())
            ),
            ("html_fallback".to_string(), None),
        ],
        "both the schema failure and the successful fallback are recorded"
    );
    let (fetch_total,): (i64,) =
        sqlx::query_as("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(fetch_total, 2);
    assert!(
        stored_document_contains(pool, &store, run.run_id, GENERIC_FALLBACK_MARKER).await?,
        "the document comes from the original page"
    );
    assert_eq!(server.request_count(), 2);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one hermetic scenario scripts two responses and asserts both recorded outcomes"
)]
async fn failed_fallback_terminates_recording_both_outcomes()
-> Result<(), Box<dyn std::error::Error>> {
    let broken_payload = json!({
        "title": "Broken schema fixture",
        "children": [],
    });
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(serde_json::to_vec(&broken_payload)?)])
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            ),
        ScriptedResponse::chunks([bytes::Bytes::from(format!(
            "<!doctype html><html><head><title>Blocked</title></head>\
             <body><p>{CHALLENGE_MARKER}</p></body></html>"
        ))])
        .with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    let address = server
        .uri("/api/v1/items/6000")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        &server.uri("/original").replace("127.0.0.1", "localhost"),
        "hacker_news",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        &config.parser,
        extractor_providers::SourceRoute::HackerNews,
        &address,
        &run,
    )
    .await?;

    let outcome: (String, Option<String>) = sqlx::query_as(
        "select status, last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        outcome.0, "failed",
        "an unusable fallback page terminates the run"
    );
    assert_eq!(outcome.1.as_deref(), Some("quality"));
    let kinds: Vec<(String, Option<String>)> = sqlx::query_as(
        "select kind, failure_class from extractor.provider_resolutions
          where run_id = $1 order by ordinal",
    )
    .bind(run.run_id)
    .fetch_all(pool)
    .await?;
    assert_eq!(
        kinds,
        vec![
            (
                "provider_attempt".to_string(),
                Some("provider_schema".to_string())
            ),
            ("html_fallback".to_string(), Some("quality".to_string())),
        ],
        "the diagnostics name both the provider failure and the fallback outcome"
    );
    let (fetch_total,): (i64,) =
        sqlx::query_as("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(fetch_total, 2);
    assert_eq!(server.request_count(), 2);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn policy_blocked_article_sends_no_second_request() -> Result<(), Box<dyn std::error::Error>>
{
    let payload = json!({
        "id": 7000,
        "title": "Policy fixture",
        "text": null,
        "url": "http://169.254.169.254/latest/meta-data/",
        "children": [],
    });
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(serde_json::to_vec(&payload)?)]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    let address = server
        .uri("/api/v1/items/7000")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        "https://news.ycombinator.com/item?id=7000",
        "hacker_news",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        &config.parser,
        extractor_providers::SourceRoute::HackerNews,
        &address,
        &run,
    )
    .await?;

    let outcome: (String, Option<String>) = sqlx::query_as(
        "select status, last_error_class from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(outcome.0, "failed");
    assert_eq!(
        outcome.1.as_deref(),
        Some("policy"),
        "a policy-blocked target terminates with the typed policy class"
    );
    let steps: Vec<(String, Option<String>)> = sqlx::query_as(
        "select kind, failure_class from extractor.provider_resolutions
          where run_id = $1 order by ordinal",
    )
    .bind(run.run_id)
    .fetch_all(pool)
    .await?;
    assert_eq!(
        steps,
        vec![
            ("provider_attempt".to_string(), None),
            ("resolved_target".to_string(), Some("policy".to_string())),
        ],
        "the blocked target is recorded on its own resolution step"
    );
    let (fetch_total,): (i64,) =
        sqlx::query_as("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(
        fetch_total, 1,
        "no request leaves for the prohibited target"
    );
    database.cleanup().await?;
    Ok(())
}
