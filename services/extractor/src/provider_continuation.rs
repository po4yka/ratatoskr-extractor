//! Post-provider continuation flows: resolved link-post retrieval, typed fall-through on
//! response-content failures, and their recorded resolution steps.

use extractor_blob_store::BlobStore;
use extractor_core::ParserConfig;
use extractor_document_ir::{DocumentIrError, HtmlDocumentInput, ParseLimits, from_html};
use extractor_eventing::{
    ResolutionStep, complete_document, fail_run, reject_quality, store_document_ir,
};
use extractor_safe_fetch::{FetchRequest, SafeFetcher};
use ratatoskr_document_contracts::DocumentAddress;

use crate::pipeline::{ProcessError, completed_fetch, fetch_retryable};

/// Resolves one link-post run through its canonical external article URL.
///
/// The provider payload stays recorded first and the discussion conversion is discarded; the
/// article is fetched through the ordinary bounded path, parsed by the shared HTML parser, and
/// completed under the provider candidate decisions with their strategy name kept.
#[allow(
    clippy::too_many_lines,
    reason = "each bounded failure arm records its own resolution step before the terminal call"
)]
pub(crate) async fn resolve_external_article(
    parser: &ParserConfig,
    pool: &sqlx::PgPool,
    store: &BlobStore,
    retriever: &SafeFetcher,
    run: &extractor_eventing::QueuedRun,
    extraction: extractor_providers::ProviderExtraction,
    target: &str,
) -> Result<(), ProcessError> {
    let mut steps = vec![ResolutionStep {
        ordinal: 0,
        kind: "provider_attempt",
        outcome: Some("ok"),
        failure_class: None,
        resolved_url: None,
    }];
    let fetched = match retriever.fetch(FetchRequest::new(target)).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "resolved article fetch failed");
            let policy_blocked = matches!(
                error,
                extractor_safe_fetch::SafeFetchError::AddressPolicy
                    | extractor_safe_fetch::SafeFetchError::Url(_)
            );
            let (failure_class, retryable) = if policy_blocked {
                ("policy", false)
            } else {
                ("fetch", fetch_retryable(&error))
            };
            steps.push(ResolutionStep {
                ordinal: 1,
                kind: "resolved_target",
                outcome: Some("failed"),
                failure_class: Some(failure_class),
                resolved_url: Some(target),
            });
            fail_run(pool, run.run_id, failure_class, retryable, &steps).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    if fetched.media_type != "text/html" {
        tracing::info!(run_id = %run.run_id, media_type = %fetched.media_type,
            "resolved article did not answer with HTML");
        steps.push(ResolutionStep {
            ordinal: 1,
            kind: "resolved_target",
            outcome: Some("failed"),
            failure_class: Some("unsupported_media"),
            resolved_url: Some(fetched.final_url.as_str()),
        });
        eprintln!("RESOLVE-DIAG: unsupported media");
        fail_run(pool, run.run_id, "unsupported_media", false, &steps).await?;
        metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
        return Ok(());
    }
    let source_path = match store.verify(&fetched.artifact).await {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "resolved artifact verification failed");
            steps.push(ResolutionStep {
                ordinal: 1,
                kind: "resolved_target",
                outcome: Some("failed"),
                failure_class: Some("artifact"),
                resolved_url: Some(fetched.final_url.as_str()),
            });
            fail_run(pool, run.run_id, "artifact", false, &steps).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let article_fetch = completed_fetch(&fetched);
    // The article fetch row and its raw-source artifact are persisted atomically by
    // `complete_document` below; recording them here as well would double-insert both.
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
    let parsed = tokio::task::spawn_blocking(move || {
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
    let article = match parsed {
        Ok(article) => article,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "resolved article conversion failed");
            steps.push(ResolutionStep {
                ordinal: 1,
                kind: "resolved_target",
                outcome: Some("failed"),
                failure_class: Some("parse"),
                resolved_url: Some(fetched.final_url.as_str()),
            });
            fail_run(pool, run.run_id, "parse", false, &steps).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    steps.push(ResolutionStep {
        ordinal: 1,
        kind: "resolved_target",
        outcome: Some("ok"),
        failure_class: None,
        resolved_url: Some(fetched.final_url.as_str()),
    });
    let ir_blob = store_document_ir(store, &article.document).await?;
    complete_document(
        pool,
        run.run_id,
        &article.document,
        &ir_blob,
        &article_fetch,
        &extraction.candidates,
        &steps,
    )
    .await?;
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded").increment(1);
    Ok(())
}

/// Makes exactly one ordinary generic HTML attempt on the original normalized URL after a typed
/// provider failure. The attempt passes the ordinary candidates and quality gates like any other
/// HTML source, and the persisted resolution records both outcomes whatever it does.
#[allow(
    clippy::too_many_lines,
    reason = "each bounded failure arm records its own resolution step before the terminal call"
)]
pub(crate) async fn fallback_to_generic_html(
    parser: &ParserConfig,
    pool: &sqlx::PgPool,
    store: &BlobStore,
    retriever: &SafeFetcher,
    run: &extractor_eventing::QueuedRun,
    provider_class: &'static str,
) -> Result<(), ProcessError> {
    let mut steps = vec![ResolutionStep {
        ordinal: 0,
        kind: "provider_attempt",
        outcome: Some("failed"),
        failure_class: Some(provider_class),
        resolved_url: None,
    }];
    let fetched = match retriever.fetch(FetchRequest::new(&run.url)).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "fallback fetch failed");
            steps.push(ResolutionStep {
                ordinal: 1,
                kind: "html_fallback",
                outcome: Some("failed"),
                failure_class: Some("fetch"),
                resolved_url: None,
            });
            fail_run(pool, run.run_id, "fetch", fetch_retryable(&error), &steps).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    if fetched.media_type != "text/html" {
        tracing::info!(run_id = %run.run_id, media_type = %fetched.media_type,
            "fallback target did not answer with HTML");
        steps.push(ResolutionStep {
            ordinal: 1,
            kind: "html_fallback",
            outcome: Some("failed"),
            failure_class: Some("unsupported_media"),
            resolved_url: None,
        });
        fail_run(pool, run.run_id, "unsupported_media", false, &steps).await?;
        metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
        return Ok(());
    }
    let source_path = match store.verify(&fetched.artifact).await {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(
                run_id = %run.run_id, error = %error, "fallback artifact verification failed"
            );
            steps.push(ResolutionStep {
                ordinal: 1,
                kind: "html_fallback",
                outcome: Some("failed"),
                failure_class: Some("artifact"),
                resolved_url: None,
            });
            fail_run(pool, run.run_id, "artifact", false, &steps).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    let fallback_fetch = completed_fetch(&fetched);
    // The fallback fetch row and its raw-source artifact persist atomically with the terminal
    // outcome below; recording them separately would double-insert both rows.
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
    let parsed = tokio::task::spawn_blocking(move || {
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
    let extraction = match parsed {
        Ok(extraction) => extraction,
        Err(DocumentIrError::LowQuality { candidates }) => {
            tracing::info!(run_id = %run.run_id, "fallback content missed quality thresholds");
            steps.push(ResolutionStep {
                ordinal: 1,
                kind: "html_fallback",
                outcome: Some("failed"),
                failure_class: Some("quality"),
                resolved_url: None,
            });
            reject_quality(
                pool,
                run.run_id,
                &fallback_fetch,
                &candidates,
                "quality",
                &steps,
            )
            .await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
        Err(error) => {
            tracing::warn!(run_id = %run.run_id, error = %error, "fallback conversion failed");
            steps.push(ResolutionStep {
                ordinal: 1,
                kind: "html_fallback",
                outcome: Some("failed"),
                failure_class: Some("parse"),
                resolved_url: None,
            });
            fail_run(pool, run.run_id, "parse", false, &steps).await?;
            metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "failed").increment(1);
            return Ok(());
        }
    };
    steps.push(ResolutionStep {
        ordinal: 1,
        kind: "html_fallback",
        outcome: Some("ok"),
        failure_class: None,
        resolved_url: None,
    });
    let ir_blob = store_document_ir(store, &extraction.document).await?;
    complete_document(
        pool,
        run.run_id,
        &extraction.document,
        &ir_blob,
        &fallback_fetch,
        &extraction.candidates,
        &steps,
    )
    .await?;
    metrics::counter!("ratatoskr_extractor_runs_total", "outcome" => "succeeded").increment(1);
    Ok(())
}

/// Conservative same-origin evidence for provider resolution: true unless both URLs parse to
/// hosts sharing a registrable domain. Unparseable input conservatively counts as pointing back.
pub(crate) fn points_back_to_source(source_url: &str, target_url: &str) -> bool {
    let (Ok(source), Ok(target)) = (url::Url::parse(source_url), url::Url::parse(target_url))
    else {
        return true;
    };
    let (Some(source_host), Some(target_host)) = (source.host_str(), target.host_str()) else {
        return true;
    };
    registrable_domain(source_host) == registrable_domain(target_host)
}

// TODO(po4yka): public-suffix awareness; last-two-labels misclassifies multi-part public
// suffixes such as co.uk hosts.
fn registrable_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let mut labels = host.split('.').rev();
    let tld = labels.next().unwrap_or_default();
    match labels.next() {
        Some(second) => format!("{second}.{tld}"),
        None => tld.to_owned(),
    }
}
