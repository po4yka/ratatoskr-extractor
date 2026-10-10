//! PDF runs: bytes fetched from a URL and bytes another service stored share one finishing path.

use extractor_blob_store::BlobStore;
use extractor_core::PdfConfig;
use extractor_eventing::{
    complete_blob_document, complete_document, fail_run, reject_blob_quality, reject_quality,
    store_document_ir,
};
use extractor_pdf::{PdfDocumentInput, PdfError, PdfParseLimits, from_pdf};
use extractor_safe_fetch::FetchResult;
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::BlobRef;

use crate::ProcessError;
use crate::pipeline::completed_fetch;

/// Completes one run whose verified bytes are a PDF document.
pub(crate) async fn complete_pdf(
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
            fail_run(pool, run.run_id, "artifact", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let bytes = bytes::Bytes::from(tokio::fs::read(source_path).await?);
    let address = DocumentAddress::parse(fetched.final_url.as_str())
        .map_err(|_| ProcessError::DocumentIdentity)?;
    let raw = fetched.artifact.clone();
    finish_pdf(pool, store, pdf, run, &raw, address, bytes, Some(&fetched)).await
}

/// Completes one run whose PDF bytes another service stored.
///
/// The peer store verifies owner, digest and length; the verified bytes are read once, copied into
/// the extractor's own store (which re-hashes them) and parsed from that same buffer, so the
/// document describes exactly the bytes the reference names. The peer root is never written.
pub(crate) async fn complete_blob_pdf(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    telegram: &BlobStore,
    pdf: &PdfConfig,
    run: &extractor_eventing::QueuedRun,
    blob: &BlobRef,
) -> Result<(), ProcessError> {
    if blob.media_type.as_str() != "application/pdf" {
        return fail_blob_run(pool, run, "unsupported_media").await;
    }
    let peer_path = match telegram.verify(blob).await {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "peer blob verification failed");
            return fail_blob_run(pool, run, blob_failure_class(&error)).await;
        }
    };
    if usize::try_from(blob.length_bytes).map_or(true, |length| length > pdf.max_input_bytes) {
        tracing::info!(run_id = %run.run_id, "peer PDF exceeds the input limit");
        return fail_blob_run(pool, run, "parse").await;
    }
    let bytes = match tokio::fs::read(&peer_path).await {
        Ok(bytes) => bytes::Bytes::from(bytes),
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "peer blob could not be read");
            return fail_blob_run(pool, run, "blob_unreadable").await;
        }
    };
    let raw = store
        .store(
            "application/pdf",
            futures_util::stream::iter([Ok::<_, std::io::Error>(bytes.clone())]),
        )
        .await?;
    if raw.digest != blob.digest || raw.length_bytes != blob.length_bytes {
        tracing::warn!(run_id = %run.run_id, "peer blob changed between verification and copy");
        return fail_blob_run(pool, run, "blob_mismatch").await;
    }
    let address =
        DocumentAddress::parse(run.url.as_str()).map_err(|_| ProcessError::DocumentIdentity)?;
    finish_pdf(pool, store, pdf, run, &raw, address, bytes, None).await
}

/// Maps a peer-store refusal to the stable failure class recorded on the run.
const fn blob_failure_class(error: &extractor_blob_store::BlobStoreError) -> &'static str {
    use extractor_blob_store::BlobStoreError;
    match error {
        BlobStoreError::WrongOwner => "blob_owner",
        BlobStoreError::Missing => "blob_missing",
        BlobStoreError::Io(_) => "blob_unreadable",
        _ => "blob_mismatch",
    }
}

async fn fail_blob_run(
    pool: &sqlx::PgPool,
    run: &extractor_eventing::QueuedRun,
    class: &str,
) -> Result<(), ProcessError> {
    fail_run(pool, run.run_id, class, false, &[]).await?;
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
    Ok(())
}

/// Parses verified PDF bytes once and commits the terminal state.
///
/// `fetched` is present for a URL run, whose fetch facts are committed with the result, and absent
/// for a blob run, which has none.
#[allow(
    clippy::too_many_arguments,
    reason = "the pipeline owns one handle for each process resource and the parse inputs"
)]
async fn finish_pdf(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    pdf: &PdfConfig,
    run: &extractor_eventing::QueuedRun,
    raw: &BlobRef,
    address: DocumentAddress,
    bytes: bytes::Bytes,
    fetched: Option<&FetchResult>,
) -> Result<(), ProcessError> {
    let limits = PdfParseLimits {
        max_input_bytes: pdf.max_input_bytes,
        max_pages: pdf.max_pages,
        max_text_bytes: pdf.max_text_bytes,
    };
    let document_id = run.document_id;
    let source_blob = raw.clone();
    let parse_started = std::time::Instant::now();
    // The PDF parser panics on hostile input; `from_pdf` contains that at its own boundary, and
    // this join converts any escaped panic into the typed process failure.
    let parsed = tokio::task::spawn_blocking(move || {
        from_pdf(
            PdfDocumentInput {
                document_id,
                source_address: address,
                source_blob,
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
            match fetched {
                Some(fetched) => {
                    let fetch = completed_fetch(fetched);
                    reject_quality(
                        pool,
                        run.run_id,
                        &fetch,
                        &candidates,
                        "pdf_no_text_layer",
                        &[],
                    )
                    .await?;
                }
                None => {
                    reject_blob_quality(
                        pool,
                        run.run_id,
                        raw,
                        &candidates,
                        "pdf_no_text_layer",
                        &[],
                    )
                    .await?;
                }
            }
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(PdfError::Encrypted) => {
            tracing::info!(run_id = %run.run_id, "PDF requires a password");
            fail_run(pool, run.run_id, "pdf_encrypted", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "PDF extraction failed");
            fail_run(pool, run.run_id, "parse", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let ir_blob = store_document_ir(store, &extraction.document).await?;
    match fetched {
        Some(fetched) => {
            let fetch = completed_fetch(fetched);
            complete_document(
                pool,
                run.run_id,
                &extraction.document,
                &ir_blob,
                &fetch,
                &extraction.candidates,
                &[],
            )
            .await?;
        }
        None => {
            complete_blob_document(
                pool,
                run.run_id,
                &extraction.document,
                &ir_blob,
                raw,
                &extraction.candidates,
                &[],
            )
            .await?;
        }
    }
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded").increment(1);
    Ok(())
}
