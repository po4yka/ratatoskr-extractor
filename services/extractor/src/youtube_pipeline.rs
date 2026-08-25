//! The `YouTube` completion flow: two justified fetches, typed degradation, transcript
//! conversion with extractor-owned timing diagnostics, and best-effort gated archival.

use extractor_blob_store::BlobStore;
use extractor_core::{ParserConfig, YoutubeConfig};
use extractor_document_ir::{CandidateDecision, evaluate_plain_text};
use extractor_eventing::{
    complete_document, fail_run, record_fetch, reject_quality, store_document_ir,
};
use extractor_safe_fetch::{FetchRequest, SafeFetcher};
use extractor_youtube::{
    CanonicalWatchAddress, TranscriptInput, VideoIdentity, YoutubeLimits, extract_player_response,
    from_transcript, parse_timedtext, resolve_identity, select_track,
};
use ratatoskr_document_contracts::DocumentAddress;

use crate::pipeline::{ProcessError, completed_fetch, fetch_retryable};
use crate::provider_continuation::fallback_to_generic_html;
use crate::youtube_media::{ArchivalOutcome, MediaArchiver, YtDlpDownloader};

/// Completes one `YouTube` run: watch page once, one validated timed-text fetch, transcript
/// conversion with extractor-owned timing diagnostics, then best-effort gated archival.
///
/// The second fetch is justified by protocol correctness - the timed-text location exists only
/// inside the fetched page. Every degradation is typed and terminal per the adapter ladder.
#[allow(
    clippy::too_many_arguments,
    reason = "the pipeline owns one handle for each process resource and finite budget"
)]
pub(crate) async fn complete_youtube(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    youtube: &YoutubeConfig,
    parser: &ParserConfig,
    retriever: &SafeFetcher,
    run: &extractor_eventing::QueuedRun,
    identity: &(VideoIdentity, CanonicalWatchAddress),
) -> Result<(), ProcessError> {
    complete_youtube_flow(
        pool,
        store,
        youtube,
        parser,
        retriever,
        run,
        identity.1.as_str(),
        identity.0.as_str(),
        None,
    )
    .await
}

/// Drives the production flow against explicit addresses so hermetic tests can substitute the
/// documented YouTube hosts with loopback equivalents without weakening any validation.
#[doc(hidden)]
#[allow(
    clippy::too_many_arguments,
    reason = "the test boundary mirrors the production signature it drives"
)]
pub async fn complete_youtube_for_test(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    youtube: &YoutubeConfig,
    parser: &ParserConfig,
    retriever: &SafeFetcher,
    run: &extractor_eventing::QueuedRun,
    watch_address: &str,
    track_address: Option<&str>,
) -> Result<(), ProcessError> {
    let video_id = resolve_identity(&run.url)
        .map(|(id, _)| id.as_str().to_owned())
        .map_err(|_| ProcessError::DocumentIdentity)?;
    complete_youtube_flow(
        pool,
        store,
        youtube,
        parser,
        retriever,
        run,
        watch_address,
        &video_id,
        track_address,
    )
    .await
}

/// One `YouTube` transcript attempt against an explicit watch address.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one function owns the two-fetch choreography, typed degradation and terminal transitions"
)]
async fn complete_youtube_flow(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    youtube: &YoutubeConfig,
    parser: &ParserConfig,
    retriever: &SafeFetcher,
    run: &extractor_eventing::QueuedRun,
    watch_address: &str,
    video_id: &str,
    track_override: Option<&str>,
) -> Result<(), ProcessError> {
    let fetched = match retriever.fetch(FetchRequest::new(watch_address)).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "watch page fetch failed");
            fail_run(pool, run.run_id, "fetch", fetch_retryable(&error), &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    if fetched.media_type != "text/html" {
        tracing::info!(run_id = %run.run_id, media_type = %fetched.media_type,
            "watch address did not answer with HTML; making one generic HTML attempt");
        record_fetch(pool, run.run_id, &completed_fetch(&fetched)).await?;
        return fallback_to_generic_html(
            parser,
            pool,
            store,
            retriever,
            run,
            "youtube_player_schema",
        )
        .await;
    }
    let source_path = match store.verify(&fetched.artifact).await {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "raw artifact verification failed");
            fail_run(pool, run.run_id, "artifact", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let bytes = tokio::fs::read(source_path).await?;
    // Adapter-fixed conversion budgets: long videos merge deterministically past the block cap.
    let limits = YoutubeLimits {
        max_page_bytes: parser.max_input_bytes,
        max_track_bytes: parser.max_input_bytes,
        max_blocks: 2_000,
        max_segments: 8_000,
        max_segment_characters: 2_000,
    };
    let parse_started = std::time::Instant::now();
    let meta = {
        // The page moves into the blocking task; non-UTF-8 bytes degrade lossily.
        let payload = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(lost) => String::from_utf8_lossy(lost.as_bytes()).into_owned(),
        };
        tokio::task::spawn_blocking(move || {
            // Copy the budgets into the blocking closure; spawn_blocking requires 'static.
            let limits = limits;
            extract_player_response(&payload, &limits)
        })
        .await?
    };
    metrics::histogram!("ratatoskr_extractor_parse_duration_seconds")
        .record(parse_started.elapsed().as_secs_f64());
    let meta = match meta {
        Ok(meta) => meta,
        Err(extractor_youtube::YoutubeError::ResourceLimit) => {
            tracing::warn!(run_id = %run.run_id, "watch page exceeded the parse budget");
            fail_run(pool, run.run_id, "parse", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(error) => {
            tracing::info!(
                run_id = %run.run_id, error = %error,
                "player response unusable; making one generic HTML attempt"
            );
            record_fetch(pool, run.run_id, &completed_fetch(&fetched)).await?;
            return fallback_to_generic_html(
                parser,
                pool,
                store,
                retriever,
                run,
                "youtube_player_schema",
            )
            .await;
        }
    };

    // Track selection validates the advertised location (HTTPS on a documented YouTube host)
    // before matching, so a foreign location is never requested.
    let track = match select_track(&meta.tracks, &youtube.transcript.languages) {
        Ok(track) => track,
        Err(extractor_youtube::YoutubeError::NoLanguageMatch) => {
            tracing::info!(run_id = %run.run_id, "no caption track matches the language preference");
            let rejected = rejected_transcript_decision(meta.title.as_deref());
            reject_quality(
                pool,
                run.run_id,
                &completed_fetch(&fetched),
                std::slice::from_ref(&rejected),
                "youtube_no_language_match",
                &[],
            )
            .await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(_) => {
            tracing::info!(run_id = %run.run_id, "video advertises no usable caption tracks");
            let rejected = rejected_transcript_decision(meta.title.as_deref());
            reject_quality(
                pool,
                run.run_id,
                &completed_fetch(&fetched),
                std::slice::from_ref(&rejected),
                "youtube_no_transcript",
                &[],
            )
            .await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    // The intermediate watch-page fetch keeps its own row and artifact; the final track fetch's
    // row and artifact are inserted by `complete_document` below, which would double-insert
    // both if recorded here as well.
    record_fetch(pool, run.run_id, &completed_fetch(&fetched)).await?;

    let track_address: std::borrow::Cow<'_, str> = match track_override {
        Some(address) => std::borrow::Cow::Borrowed(address),
        None => std::borrow::Cow::Borrowed(track.base_url.as_str()),
    };
    let track_fetch = match retriever
        .fetch(FetchRequest::new(track_address.as_ref()))
        .await
    {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "timed text fetch failed");
            fail_run(pool, run.run_id, "fetch", fetch_retryable(&error), &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    if !track_fetch.media_type.starts_with("text/") && track_fetch.media_type != "application/json"
    {
        tracing::info!(run_id = %run.run_id, media_type = %track_fetch.media_type,
            "timed-text endpoint answered with an unexpected representation");
        fail_run(pool, run.run_id, "parse", false, &[]).await?;
        metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
        return Ok(());
    }
    let track_path = match store.verify(&track_fetch.artifact).await {
        Ok(path) => path,
        Err(_error) => {
            fail_run(pool, run.run_id, "artifact", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let track_bytes = tokio::fs::read(track_path).await?;
    let segments = match parse_timedtext(&track_bytes, &limits) {
        Ok(segments) => segments,
        Err(extractor_youtube::YoutubeError::ResourceLimit) => {
            tracing::warn!(run_id = %run.run_id, "timed text exceeded the parse budget");
            fail_run(pool, run.run_id, "parse", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "timed text could not be parsed");
            fail_run(pool, run.run_id, "parse", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };

    let address =
        DocumentAddress::parse(watch_address).map_err(|_| ProcessError::DocumentIdentity)?;
    let conversion = from_transcript(
        TranscriptInput {
            document_id: run.document_id,
            source_address: address,
            source_blob: fetched.artifact.clone(),
            meta: &meta,
            segments,
            language: &track.language_code,
        },
        limits,
    );
    let extraction = match conversion {
        Ok(extraction) => extraction,
        Err(extractor_youtube::YoutubeError::Serialization(error)) => {
            return Err(ProcessError::Io(std::io::Error::other(error)));
        }
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "transcript conversion failed");
            fail_run(pool, run.run_id, "parse", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };

    let ir_blob = store_document_ir(store, &extraction.document).await?;
    complete_document(
        pool,
        run.run_id,
        &extraction.document,
        &ir_blob,
        &completed_fetch(&track_fetch),
        std::slice::from_ref(&extraction.candidate),
        &[],
    )
    .await?;
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded").increment(1);

    // Archival never influences the terminal outcome; its result lands in the timing sidecar
    // diagnostics written after the run has gone terminal-succeeded.
    let archival = archive_media(pool, store, youtube, run, video_id, watch_address).await;
    write_timing_diagnostics(
        pool,
        store,
        run.run_id,
        &extraction.sidecar,
        archival_label(&archival),
    )
    .await?;
    Ok(())
}

/// One unselected transcript-strategy decision so a rejection still records its evidence.
fn rejected_transcript_decision(title: Option<&str>) -> CandidateDecision {
    let mut decision = evaluate_plain_text(extractor_youtube::YOUTUBE_STRATEGY, &[], title);
    decision.selected = false;
    decision
}

async fn archive_media(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    youtube: &YoutubeConfig,
    run: &extractor_eventing::QueuedRun,
    video_id: &str,
    canonical_url: &str,
) -> std::io::Result<ArchivalOutcome> {
    let downloader = YtDlpDownloader::from_parts(
        &youtube.media.binary_path,
        youtube.media.timeout_secs,
        youtube.media.max_height,
    );
    MediaArchiver::new(pool, store, youtube, &downloader)
        .archive_video(run.run_id, video_id, canonical_url)
        .await
}

fn archival_label(outcome: &std::io::Result<ArchivalOutcome>) -> &'static str {
    let Ok(outcome) = outcome else {
        return "error";
    };
    match outcome {
        ArchivalOutcome::Disabled => "disabled",
        ArchivalOutcome::SkippedDuplicate => "skipped_duplicate",
        ArchivalOutcome::SkippedBudgetExhausted
        | ArchivalOutcome::SkippedBudgetRace
        | ArchivalOutcome::FailedDownload {
            class: "media_over_item_cap",
        } => "skipped_budget",
        ArchivalOutcome::Stored { .. } => "stored",
        ArchivalOutcome::FailedDownload { .. } => "failed",
    }
}

/// Persists the extractor-owned timing sidecar plus the archival label as one diagnostics
/// artifact so segment timing stays recoverable inside the extractor without entering the
/// shared Document shape.
async fn write_timing_diagnostics(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    run_id: uuid::Uuid,
    sidecar: &extractor_youtube::sidecar::TimingSidecar,
    archival: &str,
) -> Result<(), ProcessError> {
    let payload = serde_json::to_vec(&serde_json::json!({
        "timing": serde_json::to_value(sidecar).map_err(|_| ProcessError::DocumentIdentity)?,
        "archival": archival,
    }))
    .map_err(|_| ProcessError::DocumentIdentity)?;
    let reference = store
        .store(
            "application/json",
            futures_util::stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from(payload))]),
        )
        .await?;
    let owner_id: uuid::Uuid =
        sqlx::query_scalar("select owner_id from extractor.extraction_runs where run_id = $1")
            .bind(run_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| ProcessError::Io(std::io::Error::other(error)))?
            .ok_or(ProcessError::DocumentIdentity)?;
    extractor_persistence::record_artifact(
        pool,
        &extractor_persistence::ArtifactRecord {
            run_id,
            owner_id,
            kind: extractor_persistence::ArtifactKind::Diagnostics,
            reference: &reference,
        },
    )
    .await?;
    Ok(())
}
