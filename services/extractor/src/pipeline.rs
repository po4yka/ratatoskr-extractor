//! The per-run extraction pipeline shared by the worker loop and integration tests.

use extractor_blob_store::BlobStore;
use extractor_core::{ParserConfig, PdfConfig, ProvidersConfig, RenderConfig, YoutubeConfig};
use extractor_document_ir::{
    DocumentIrError, HtmlDocumentInput, HtmlExtraction, ParseLimits, from_html,
};
use extractor_eventing::{
    CompletedFetch, RenderOutcome, RenderRequestError, complete_document, fail_run, record_fetch,
    reject_quality, store_document_ir,
};

use crate::provider_continuation::{
    fallback_to_generic_html, points_back_to_source, resolve_external_article,
};
use extractor_pdf::{PdfDocumentInput, PdfError, PdfParseLimits, from_pdf};
use extractor_providers::{
    ProviderError, ProviderInput, ProviderLimits, SourceRoute, from_provider, provider_request,
};
use extractor_safe_fetch::{CacheOutcome, FetchRequest, FetchResult, SafeFetcher};
use extractor_youtube::resolve_identity;
use ratatoskr_document_contracts::DocumentAddress;

use crate::youtube_pipeline::complete_youtube;

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
    /// A provider adapter rejected its inputs.
    #[error("provider adapter failed")]
    Provider(#[from] extractor_providers::ProviderError),
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
    providers: &ProvidersConfig,
    render: &RenderConfig,
    youtube: &YoutubeConfig,
    bus: &async_nats::jetstream::Context,
    run: &extractor_eventing::QueuedRun,
) -> Result<(), ProcessError> {
    // Provider routing happens before any fetch so a mapped provider run performs exactly one
    // network operation against its native representation; unmappable URLs fall through to the
    // ordinary path with the original URL. YouTube routes resolve a video identity first; a URL
    // that names no video takes the ordinary path with the original URL unchanged.
    let route = match run.classification.as_str() {
        "hacker_news" => Some(SourceRoute::HackerNews),
        "reddit" => Some(SourceRoute::Reddit),
        _ => None,
    };
    if let Some(route) = route
        && let Some(address) = provider_request(route, &run.url)?
    {
        return complete_provider(
            pool, store, providers, retriever, parser, route, address, run,
        )
        .await;
    }
    if run.classification == "youtube" {
        return match resolve_identity(&run.url) {
            Ok(identity) => {
                complete_youtube(pool, store, youtube, parser, retriever, run, &identity).await
            }
            Err(_) => {
                fallback_to_generic_html(parser, pool, store, retriever, run, "youtube_no_video_id")
                    .await
            }
        };
    }
    let fetched = match retriever.fetch(FetchRequest::new(&run.url)).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "safe fetch failed");
            fail_run(pool, run.run_id, "fetch", fetch_retryable(&error), &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    match fetched.media_type.as_str() {
        "text/html" => complete_html(pool, store, parser, render, bus, run, fetched).await,
        "application/pdf" => complete_pdf(pool, store, pdf, run, fetched).await,
        _ => {
            fail_run(pool, run.run_id, "unsupported_media", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            Ok(())
        }
    }
}

async fn complete_html(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    parser: &ParserConfig,
    render: &RenderConfig,
    bus: &async_nats::jetstream::Context,
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
    let bytes = tokio::fs::read(source_path).await?;
    let raw_html = bytes.clone();
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
            // Deterministic escalation: only an empty-shell shape with rendering enabled may
            // escalate, and only once — this branch has no escalation of its own.
            let empty_shell = is_empty_shell(&raw_html)
                && candidates.iter().all(|c| c.metrics.text_characters < 120);
            if render.enabled
                && empty_shell
                && let Some(extraction) =
                    escalate_to_render(pool, render, bus, parser, run, &fetched).await?
            {
                let ir_blob = store_document_ir(store, &extraction.document).await?;
                let fetch = completed_fetch(&fetched);
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
                metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded")
                    .increment(1);
                return Ok(());
            }
            let fetch = completed_fetch(&fetched);
            reject_quality(pool, run.run_id, &fetch, &candidates, "quality", &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "Document IR conversion failed");
            fail_run(pool, run.run_id, "parse", false, &[]).await?;
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
        &[],
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
            fail_run(pool, run.run_id, "artifact", false, &[]).await?;
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
            reject_quality(
                pool,
                run.run_id,
                &fetch,
                &candidates,
                "pdf_no_text_layer",
                &[],
            )
            .await?;
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
    let fetch = completed_fetch(&fetched);
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
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded").increment(1);
    Ok(())
}

/// Completes one run whose classified source maps to a native provider representation.
#[allow(
    clippy::too_many_arguments,
    reason = "the pipeline owns one handle for each process resource and finite budget"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one function owns fetch, conversion, resolution branching and terminal transitions"
)]
async fn complete_provider(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    providers: &ProvidersConfig,
    retriever: &SafeFetcher,
    parser: &ParserConfig,
    route: SourceRoute,
    address: DocumentAddress,
    run: &extractor_eventing::QueuedRun,
) -> Result<(), ProcessError> {
    let fetched = match retriever.fetch(FetchRequest::new(address.as_str())).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "provider fetch failed");
            fail_run(pool, run.run_id, "fetch", fetch_retryable(&error), &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    if fetched.media_type != "application/json" {
        tracing::info!(run_id = %run.run_id, media_type = %fetched.media_type,
            "provider endpoint did not answer with JSON; making one generic HTML attempt");
        // The provider payload stays recorded and reprocessable even though the run continues
        // past it into the generic HTML attempt below.
        let provider_fetch = completed_fetch(&fetched);
        record_fetch(pool, run.run_id, &provider_fetch).await?;
        return fallback_to_generic_html(parser, pool, store, retriever, run, "provider_response")
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
    let payload_address = DocumentAddress::parse(fetched.final_url.as_str())
        .map_err(|_| ProcessError::DocumentIdentity)?;
    let raw = fetched.artifact.clone();
    let limits = ProviderLimits {
        max_input_bytes: providers.max_input_bytes,
        max_blocks: providers.max_blocks,
    };
    let document_id = run.document_id;
    let parse_started = std::time::Instant::now();
    let parsed = tokio::task::spawn_blocking(move || {
        from_provider(
            ProviderInput {
                document_id,
                source_address: payload_address,
                source_blob: raw,
                route,
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
        Err(ProviderError::LowQuality { candidates }) => {
            tracing::info!(run_id = %run.run_id, "provider content missed quality thresholds");
            let fetch = completed_fetch(&fetched);
            reject_quality(
                pool,
                run.run_id,
                &fetch,
                &candidates,
                "provider_low_quality",
                &[],
            )
            .await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(ProviderError::ResourceLimit) => {
            tracing::warn!(run_id = %run.run_id, error = "resource limit", "provider conversion failed");
            fail_run(pool, run.run_id, "parse", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(
                run_id = %run.run_id, error = %error,
                "provider conversion failed; making one generic HTML attempt"
            );
            let provider_fetch = completed_fetch(&fetched);
            record_fetch(pool, run.run_id, &provider_fetch).await?;
            return fallback_to_generic_html(
                parser,
                pool,
                store,
                retriever,
                run,
                "provider_schema",
            )
            .await;
        }
    };
    // Post-resolution guard: a payload-carried article URL that resolves back onto the source
    // host keeps the run on the ordinary self-contained completion below.
    let external_target = extraction
        .external_url
        .clone()
        .filter(|target| !points_back_to_source(&run.url, target));
    if let Some(target) = external_target.as_deref() {
        let provider_fetch = completed_fetch(&fetched);
        record_fetch(pool, run.run_id, &provider_fetch).await?;
        return resolve_external_article(parser, pool, store, retriever, run, extraction, target)
            .await;
    }
    let ir_blob = store_document_ir(store, &extraction.document).await?;
    let fetch = completed_fetch(&fetched);
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
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded").increment(1);
    Ok(())
}

/// Runs only the provider branch against an explicit address for integration tests.
///
/// Production callers always go through [`process_run`], which derives the address from the
/// claimed run's classification; external hosts cannot be reached from a hermetic test
/// environment, so tests drive this boundary with a loopback address instead.
///
/// # Errors
///
/// Returns [`ProcessError`] under the same conditions as [`process_run`].
#[doc(hidden)]
#[allow(
    clippy::too_many_arguments,
    reason = "the test boundary mirrors the production signature it drives"
)]
pub async fn complete_provider_for_test(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    providers: &ProvidersConfig,
    retriever: &SafeFetcher,
    parser: &ParserConfig,
    route: SourceRoute,
    address: &str,
    run: &extractor_eventing::QueuedRun,
) -> Result<(), ProcessError> {
    let address = DocumentAddress::parse(address).map_err(|_| ProcessError::DocumentIdentity)?;
    complete_provider(
        pool, store, providers, retriever, parser, route, address, run,
    )
    .await
}

pub(crate) fn completed_fetch(fetched: &FetchResult) -> CompletedFetch<'_> {
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

pub(crate) const fn fetch_retryable(error: &extractor_safe_fetch::SafeFetchError) -> bool {
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

/// Empty-shell evidence: hydration mount markers in bytes that yielded almost no text.
fn is_empty_shell(raw_html: &[u8]) -> bool {
    let haystack = String::from_utf8_lossy(raw_html);
    ["id=\"root\"", "id=\"app\"", "__NEXT_DATA__"]
        .iter()
        .any(|marker| haystack.contains(marker))
}

async fn escalate_to_render(
    pool: &sqlx::PgPool,
    render: &RenderConfig,
    bus: &async_nats::jetstream::Context,
    parser: &ParserConfig,
    run: &extractor_eventing::QueuedRun,
    fetched: &FetchResult,
) -> Result<Option<HtmlExtraction>, ProcessError> {
    tracing::info!(run_id = %run.run_id, "empty shell detected; escalating to browser worker");
    let command = render_job::RenderCommand {
        render_id: uuid::Uuid::now_v7(),
        operation_id: uuid::Uuid::now_v7(),
        correlation_id: format!("run:{}", run.run_id),
        tenant_user_id: uuid::Uuid::nil(),
        url: run.url.clone(),
        budgets: render_job::RenderBudgets {
            navigation_timeout_ms: render.navigation_timeout_ms,
            total_timeout_ms: render.total_timeout_ms,
            max_dom_bytes: render.max_dom_bytes,
        },
    };
    match extractor_eventing::request_render(bus, &command).await {
        Ok(RenderOutcome::Completed(completed)) => {
            let worker_store = BlobStore::new(&render.worker_blobs_root)
                .with_owner("ratatoskr-browser-worker")
                .map_err(ProcessError::Blob)?;
            let path = worker_store.verify(&completed.dom).await?;
            let rendered = tokio::fs::read(path).await?;
            let address = DocumentAddress::parse(&completed.final_url)
                .map_err(|_| ProcessError::DocumentIdentity)?;
            let document_id = run.document_id;
            let parse_limits = ParseLimits {
                max_input_bytes: parser.max_input_bytes,
                max_dom_nodes: parser.max_dom_nodes,
            };
            let parse_started = std::time::Instant::now();
            let parsed = tokio::task::spawn_blocking(move || {
                from_html(
                    HtmlDocumentInput {
                        document_id,
                        source_address: address,
                        source_blob: completed.dom.clone(),
                        bytes: &rendered,
                    },
                    parse_limits,
                )
            })
            .await?;
            metrics::histogram!("ratatoskr_extractor_parse_duration_seconds")
                .record(parse_started.elapsed().as_secs_f64());
            match parsed {
                Ok(extraction) => Ok(Some(extraction)),
                Err(DocumentIrError::LowQuality { candidates }) => {
                    let fetch = completed_fetch(fetched);
                    reject_quality(pool, run.run_id, &fetch, &candidates, "quality", &[]).await?;
                    metrics::counter!(
                        "ratatoskr_extractor_runs_total",
                        "outcome" => "failed"
                    )
                    .increment(1);
                    Ok(None)
                }
                Err(error) => {
                    tracing::warn!(run_id = %run.run_id, error = %error, "rendered DOM conversion failed");
                    fail_run(pool, run.run_id, "parse", false, &[]).await?;
                    metrics::counter!(
                        "ratatoskr_extractor_runs_total",
                        "outcome" => "failed"
                    )
                    .increment(1);
                    Ok(None)
                }
            }
        }
        Ok(RenderOutcome::Failed(failed)) => {
            fail_run(pool, run.run_id, failed.class.as_str(), false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            Ok(None)
        }
        Err(RenderRequestError::Timeout) => {
            fail_run(pool, run.run_id, "navigation_timeout", false, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            Ok(None)
        }
        Err(RenderRequestError::Transport(error)) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "render transport failed");
            fail_run(pool, run.run_id, "fetch", true, &[]).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            Ok(None)
        }
    }
}
