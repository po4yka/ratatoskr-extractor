//! The per-run extraction pipeline shared by the worker loop and integration tests.

use extractor_blob_store::BlobStore;
use extractor_core::{ParserConfig, PdfConfig};
use extractor_document_ir::{DocumentIrError, HtmlDocumentInput, ParseLimits, from_html};
use extractor_eventing::{
    CompletedFetch, complete_document, fail_run, reject_quality, store_document_ir,
};
use extractor_pdf::{PdfDocumentInput, PdfError, PdfParseLimits, from_pdf};
use extractor_safe_fetch::{CacheOutcome, FetchRequest, FetchResult, SafeFetcher};
use ratatoskr_document_contracts::DocumentAddress;

/// Why the process or one pipeline step failed.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// Artifact storage initialization failed.
    #[error("artifact storage initialization failed")]
    Blob(#[from] extractor_blob_store::BlobStoreError),
    /// Safe fetch initialization failed.
    #[error("safe fetch initialization failed")]
    Fetch(#[from] extractor_safe_fetch::SafeFetchError),
    /// Telemetry initialization failed.
    #[error("telemetry initialization failed")]
    Telemetry(#[from] extractor_telemetry::TelemetryError),
    /// Admin listener failed.
    #[error("admin listener failed")]
    Io(#[from] std::io::Error),
    /// Database initialization failed.
    #[error("database initialization failed")]
    Database(#[from] extractor_persistence::PersistenceError),
    /// Event bus initialization failed.
    #[error("event bus initialization failed")]
    Bus(#[from] extractor_eventing::PublishError),
    /// Event pipeline failed.
    #[error("event pipeline failed")]
    Eventing(#[from] extractor_eventing::ConsumeError),
    /// Background task failed.
    #[error("background task failed")]
    Join(#[from] tokio::task::JoinError),
    /// A required background task stopped.
    #[error("a required background task stopped")]
    BackgroundStopped,
    /// Document IR identity is invalid.
    #[error("Document IR identity is invalid")]
    DocumentIdentity,
}

#[allow(
    clippy::too_many_arguments,
    reason = "the pipeline owns one handle for each process resource and finite budget"
)]
/// Runs one claimed extraction run from fetch through durable completion.
///
/// # Errors
///
/// Returns [`ProcessError`] when persistence or artifact storage fails; extraction failures are
/// recorded on the run instead of surfacing here.
pub async fn process_run(
    pool: &sqlx::PgPool,
    retriever: &SafeFetcher,
    store: &BlobStore,
    parser: &ParserConfig,
    pdf: &PdfConfig,
    run: &extractor_eventing::QueuedRun,
) -> Result<(), ProcessError> {
    let fetched = match retriever.fetch(FetchRequest::new(&run.url)).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "safe fetch failed");
            fail_run(pool, run.run_id, "fetch", fetch_retryable(&error)).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    match fetched.media_type.as_str() {
        "text/html" => complete_html(pool, store, parser, run, fetched).await,
        "application/pdf" => complete_pdf(pool, store, pdf, run, fetched).await,
        _ => {
            fail_run(pool, run.run_id, "unsupported_media", false).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            Ok(())
        }
    }
}

async fn complete_html(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    parser: &ParserConfig,
    run: &extractor_eventing::QueuedRun,
    fetched: FetchResult,
) -> Result<(), ProcessError> {
    let source_path = match store.verify(&fetched.artifact).await {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "raw artifact verification failed");
            fail_run(pool, run.run_id, "artifact", false).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let bytes = tokio::fs::read(source_path).await?;
    let address = DocumentAddress::parse(fetched.final_url.as_str())
        .map_err(|_| ProcessError::DocumentIdentity)?;
    let raw = fetched.artifact.clone();
    let limits = ParseLimits {
        max_input_bytes: parser.max_input_bytes,
        max_dom_nodes: parser.max_dom_nodes,
    };
    let document_id = run.document_id;
    let parse_started = std::time::Instant::now();
    let document = tokio::task::spawn_blocking(move || {
        from_html(
            HtmlDocumentInput {
                document_id,
                source_address: address,
                source_blob: raw,
                bytes: &bytes,
            },
            limits,
        )
    })
    .await?;
    metrics::histogram!("ratatoskr_extractor_parse_duration_seconds")
        .record(parse_started.elapsed().as_secs_f64());
    let extraction = match document {
        Ok(extraction) => extraction,
        Err(DocumentIrError::LowQuality { candidates }) => {
            let fetch = completed_fetch(&fetched);
            reject_quality(pool, run.run_id, &fetch, &candidates, "quality").await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "Document IR conversion failed");
            fail_run(pool, run.run_id, "parse", false).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let ir_blob = store_document_ir(store, &extraction.document).await?;
    let fetch = completed_fetch(&fetched);
    complete_document(
        pool,
        run.run_id,
        &extraction.document,
        &ir_blob,
        &fetch,
        &extraction.candidates,
    )
    .await?;
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded").increment(1);
    Ok(())
}

/// Completes one run whose verified bytes are a PDF document.
async fn complete_pdf(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    pdf: &PdfConfig,
    run: &extractor_eventing::QueuedRun,
    fetched: FetchResult,
) -> Result<(), ProcessError> {
    let source_path = match store.verify(&fetched.artifact).await {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "raw artifact verification failed");
            fail_run(pool, run.run_id, "artifact", false).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let bytes = tokio::fs::read(source_path).await?;
    let address = DocumentAddress::parse(fetched.final_url.as_str())
        .map_err(|_| ProcessError::DocumentIdentity)?;
    let raw = fetched.artifact.clone();
    let limits = PdfParseLimits {
        max_input_bytes: pdf.max_input_bytes,
        max_pages: pdf.max_pages,
        max_text_bytes: pdf.max_text_bytes,
    };
    let document_id = run.document_id;
    let parse_started = std::time::Instant::now();
    // The PDF parser panics on hostile input; `from_pdf` contains that at its own boundary, and
    // this join converts any escaped panic into the typed process failure.
    let parsed = tokio::task::spawn_blocking(move || {
        from_pdf(
            PdfDocumentInput {
                document_id,
                source_address: address,
                source_blob: raw,
                bytes: &bytes,
            },
            limits,
        )
    })
    .await?;
    metrics::histogram!("ratatoskr_extractor_parse_duration_seconds")
        .record(parse_started.elapsed().as_secs_f64());
    let extraction = match parsed {
        Ok(extraction) => extraction,
        Err(PdfError::NoTextLayer { candidates }) => {
            tracing::info!(run_id = %run.run_id, "PDF has no text layer; recording degraded outcome");
            let fetch = completed_fetch(&fetched);
            reject_quality(pool, run.run_id, &fetch, &candidates, "pdf_no_text_layer").await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(PdfError::Encrypted) => {
            tracing::info!(run_id = %run.run_id, "PDF requires a password");
            fail_run(pool, run.run_id, "pdf_encrypted", false).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "PDF extraction failed");
            fail_run(pool, run.run_id, "parse", false).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let ir_blob = store_document_ir(store, &extraction.document).await?;
    let fetch = completed_fetch(&fetched);
    complete_document(
        pool,
        run.run_id,
        &extraction.document,
        &ir_blob,
        &fetch,
        &extraction.candidates,
    )
    .await?;
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded").increment(1);
    Ok(())
}

fn completed_fetch(fetched: &FetchResult) -> CompletedFetch<'_> {
    CompletedFetch {
        final_url: fetched.final_url.as_str(),
        http_status: fetched.status,
        media_type: &fetched.media_type,
        wire_bytes: fetched.wire_bytes,
        decoded_bytes: fetched.decoded_bytes,
        attempts: fetched.metadata.attempts,
        cache_outcome: match fetched.metadata.cache_outcome {
            CacheOutcome::Fresh => "fresh",
            CacheOutcome::Revalidated => "revalidated",
        },
        etag: fetched.metadata.etag.as_deref(),
        last_modified: fetched.metadata.last_modified.as_deref(),
        raw_blob: &fetched.artifact,
    }
}

const fn fetch_retryable(error: &extractor_safe_fetch::SafeFetchError) -> bool {
    matches!(
        error,
        extractor_safe_fetch::SafeFetchError::Transport
            | extractor_safe_fetch::SafeFetchError::Dns
            | extractor_safe_fetch::SafeFetchError::TimeoutTotal
            | extractor_safe_fetch::SafeFetchError::TimeoutFirstByte
            | extractor_safe_fetch::SafeFetchError::TimeoutReadIdle
            | extractor_safe_fetch::SafeFetchError::Overloaded
    )
}
