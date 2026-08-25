//! `YouTube` transcript pipeline: classified runs complete from exactly two scripted fetches,
//! carry extractor-owned timing diagnostics, and reject trackless videos with a typed class.

use extractor_blob_store::BlobStore;
use extractor_core::ExtractorConfig;
use extractor_eventing::claim_queued_run;
use extractor_persistence::test_support::TestDatabase;
use extractor_safe_fetch::SafeFetcher;
use extractor_test_support::{ScriptedResponse, ScriptedServer, TemporaryBlobRoot};
use ratatoskr_identifiers::{BlobRef, DocumentId};
use serde_json::json;

const TRANSCRIPT_MARKER: &str = "transcript_segment_fixture_marker";

fn watch_page() -> String {
    "<!DOCTYPE html><html><head><title>Fixture video - YouTube</title></head><body>\
         <script>var ytInitialPlayerResponse = {\
         \"videoDetails\": {\"title\": \"Fixture Video Title\", \
         \"author\": \"Example Channel\", \"lengthSeconds\": \"2123\"}, \
         \"captions\": {\"playerCaptionTracks\": [{\
         \"baseUrl\": \"https://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ\", \
         \"languageCode\": \"en\", \
         \"name\": {\"simpleText\": \"English\"}}]}}};</script>\
         </body></html>"
        .to_owned()
}

fn timed_text() -> String {
    format!(
        "<transcript>\
         <text start=\"0.5\" dur=\"2.0\">First {TRANSCRIPT_MARKER} words</text>\
         <text start=\"3.0\" dur=\"1.5\">Second segment continues the recorded transcript</text>\
         </transcript>"
    )
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one hermetic scenario starts servers, queues one run and verifies every persisted outcome"
)]
async fn youtube_run_completes_from_exactly_two_scripted_fetches()
-> Result<(), Box<dyn std::error::Error>> {
    // Server order matters: the watch page embeds the timed-text location.
    let track_server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(timed_text())]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/xml"),
        ),
    ])
    .await?;
    let track_url = track_server
        .uri("/api/timedtext")
        .replace("127.0.0.1", "localhost");

    let watch_server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(watch_page())]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
    ])
    .await?;
    let watch_address = watch_server.uri("/watch").replace("127.0.0.1", "localhost");

    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![watch_server.port(), track_server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;

    queue_direct(
        pool,
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        "youtube",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::complete_youtube_for_test(
        pool,
        &store,
        &config.youtube,
        &config.parser,
        &fetcher,
        &run,
        &watch_address,
        Some(&track_url),
    )
    .await?;

    let (status,): (String,) =
        sqlx::query_as("select status from extractor.extraction_runs where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    if status != "succeeded" {
        let class: Option<String> = sqlx::query_scalar(
            "select last_error_class from extractor.extraction_runs where run_id = $1",
        )
        .bind(run.run_id)
        .fetch_one(pool)
        .await?;
        return Err(format!("run did not succeed: status={status} class={class:?}").into());
    }

    let (fetch_count,): (i64,) =
        sqlx::query_as("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(
        fetch_count, 2,
        "exactly one watch-page and one timed-text request"
    );

    let kinds: Vec<(String, i64)> = sqlx::query_as(
        "select kind, count(*) from extractor.artifacts where run_id = $1 group by kind order by kind",
    )
    .bind(run.run_id)
    .fetch_all(pool)
    .await?;
    let kind_map = kinds
        .iter()
        .cloned()
        .collect::<std::collections::HashMap<String, i64>>();
    assert_eq!(kind_map.get("document_ir"), Some(&1));
    assert_eq!(kind_map.get("diagnostics"), Some(&1));

    let stored = stored_blob(pool, &store, run.run_id, "document_ir").await?;
    assert!(
        stored.contains(TRANSCRIPT_MARKER),
        "published document carries the transcript text"
    );

    let diagnostics = stored_blob(pool, &store, run.run_id, "diagnostics").await?;
    assert!(
        diagnostics.contains("\"language\":\"en\""),
        "sidecar records the selected language: {diagnostics}"
    );
    assert!(
        diagnostics.contains("\"segment_count\":2"),
        "sidecar records segment coverage: {diagnostics}"
    );
    assert!(
        diagnostics.contains("\"start_ms\":500"),
        "sidecar preserves millisecond timing: {diagnostics}"
    );
    assert!(
        diagnostics.contains("Fixture Video Title"),
        "sidecar carries video metadata: {diagnostics}"
    );

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn video_without_tracks_is_rejected_with_typed_class()
-> Result<(), Box<dyn std::error::Error>> {
    let track_server = ScriptedServer::start(vec![]).await?;
    let watch_server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(
            "<!DOCTYPE html><html><body><script>var ytInitialPlayerResponse = {\
         \"videoDetails\": {\"title\": \"No Captions\", \"author\": \"Chan\", \
         \"lengthSeconds\": \"10\"}, \"captions\": {\"playerCaptionTracks\": []}};</script>\
         </body></html>",
        )])
        .with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
    ])
    .await?;
    let watch_address = watch_server.uri("/watch").replace("127.0.0.1", "localhost");

    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![watch_server.port(), track_server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;

    queue_direct(
        pool,
        "https://www.youtube.com/watch?v=noCaptionsX",
        "youtube",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;

    extractor_service::complete_youtube_for_test(
        pool,
        &store,
        &config.youtube,
        &config.parser,
        &fetcher,
        &run,
        &watch_address,
        None,
    )
    .await?;

    let row: (Option<String>, String) = sqlx::query_as(
        "select last_error_class, status from extractor.extraction_runs where run_id = $1",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        row,
        (
            Some("youtube_no_transcript".to_owned()),
            "failed".to_owned()
        )
    );

    database.cleanup().await?;
    Ok(())
}

/// Reads one stored artifact's bytes back through its recorded `BlobRef` facts.
async fn stored_blob(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    run_id: uuid::Uuid,
    kind: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let row: (String, String, String, i64) = sqlx::query_as(
        "select owner_service, digest_hex, media_type, length_bytes
           from extractor.artifacts where run_id = $1 and kind = $2",
    )
    .bind(run_id)
    .bind(kind)
    .fetch_one(pool)
    .await?;
    let reference: BlobRef = serde_json::from_value(json!({
        "owner_service": row.0,
        "digest": {"algorithm": "sha256", "hex": row.1},
        "media_type": row.2,
        "length_bytes": row.3,
    }))?;
    let path = store.verify(&reference).await?;
    Ok(String::from_utf8(tokio::fs::read(path).await?)?)
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
         values ($1, $2, $3, $3, $3, 'www.youtube.com', $4, transaction_timestamp())",
    )
    .bind(source_id)
    .bind(owner_id)
    .bind(url)
    .bind(classification)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.extraction_runs
             (run_id, command_id, operation_id, owner_id, correlation_id, source_id, document_id,
              status, policy_version, normalizer_version, parser_version, queued_at)
         values ($1, $2, $3, $4, $5, $6, $7, 'queued', 'ssrf-v1', 'url-v1', 'youtube-v1',
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
