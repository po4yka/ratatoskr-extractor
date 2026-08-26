//! Offline corpus performance measurement and threshold checks.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{CASE_NAMES, CorpusError, document_for_case};

/// One measured report over the committed successful corpus cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerformanceReport {
    /// Number of distinct corpus cases in one iteration.
    pub case_count: usize,
    /// Number of full corpus iterations measured after warmup.
    pub iterations: usize,
    /// Successful documents converted per second.
    pub throughput_documents_per_second: f64,
    /// Median per-document conversion latency in microseconds.
    pub p50_microseconds: u128,
    /// 95th-percentile per-document conversion latency in microseconds.
    pub p95_microseconds: u128,
    /// Peak RSS collected by the native platform wrapper, normalized to KiB.
    pub max_rss_kib: u64,
}

/// Explicit ceilings committed with the performance baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerformanceBaseline {
    /// Reference report generated from the committed corpus.
    pub report: PerformanceReport,
    /// Minimum allowed throughput in documents per second.
    pub min_throughput_documents_per_second: f64,
    /// Maximum allowed p95 conversion latency.
    pub max_p95_microseconds: u128,
    /// Maximum allowed peak RSS; 786432 KiB is the deployment `MemoryHigh` budget.
    pub max_rss_kib: u64,
}

/// A report crossed a named committed threshold.
#[derive(Debug, thiserror::Error)]
pub enum PerformanceError {
    /// A corpus conversion did not produce a Document IR.
    #[error("performance corpus case {case} failed: {source}")]
    Extraction {
        /// Named corpus case.
        case: String,
        /// Conversion failure.
        #[source]
        source: CorpusError,
    },
    /// No measurement iterations were requested.
    #[error("performance iterations must be greater than zero")]
    ZeroIterations,
    /// The requested run cannot be represented safely in floating-point throughput metrics.
    #[error("performance run contains too many documents to report")]
    TooManyDocuments,
    /// Throughput fell below the committed floor.
    #[error(
        "throughput_documents_per_second observed {observed:.3}, allowed at least {allowed:.3}"
    )]
    Throughput {
        /// Observed value.
        observed: f64,
        /// Allowed lower bound.
        allowed: f64,
    },
    /// P95 latency exceeded the committed ceiling.
    #[error("p95_microseconds observed {observed}, allowed at most {allowed}")]
    P95 {
        /// Observed value.
        observed: u128,
        /// Allowed upper bound.
        allowed: u128,
    },
    /// Peak resident memory exceeded the committed ceiling.
    #[error("max_rss_kib observed {observed}, allowed at most {allowed}")]
    Memory {
        /// Observed value.
        observed: u64,
        /// Allowed upper bound.
        allowed: u64,
    },
}

/// Measures each committed corpus case for `iterations` complete iterations.
///
/// # Errors
///
/// Returns [`PerformanceError`] if an input conversion fails or `iterations` is zero.
pub fn measure(iterations: usize, max_rss_kib: u64) -> Result<PerformanceReport, PerformanceError> {
    if iterations == 0 {
        return Err(PerformanceError::ZeroIterations);
    }
    for case in CASE_NAMES {
        document_for_case(case).map_err(|source| PerformanceError::Extraction {
            case: case.to_owned(),
            source,
        })?;
    }
    let mut samples = Vec::with_capacity(iterations * CASE_NAMES.len());
    let total_start = Instant::now();
    for _ in 0..iterations {
        for case in CASE_NAMES {
            let started = Instant::now();
            document_for_case(case).map_err(|source| PerformanceError::Extraction {
                case: case.to_owned(),
                source,
            })?;
            samples.push(started.elapsed().as_micros());
        }
    }
    samples.sort_unstable();
    let elapsed_seconds = total_start.elapsed().as_secs_f64();
    let total_documents =
        u32::try_from(samples.len()).map_err(|_| PerformanceError::TooManyDocuments)?;
    Ok(PerformanceReport {
        case_count: CASE_NAMES.len(),
        iterations,
        throughput_documents_per_second: f64::from(total_documents) / elapsed_seconds,
        p50_microseconds: percentile(&samples, 50),
        p95_microseconds: percentile(&samples, 95),
        max_rss_kib,
    })
}

/// Rejects every measurement that exceeds one committed budget.
///
/// # Errors
///
/// Returns [`PerformanceError`] naming the first exceeded metric and its ceiling.
pub fn check(
    report: &PerformanceReport,
    baseline: &PerformanceBaseline,
) -> Result<(), PerformanceError> {
    if report.throughput_documents_per_second < baseline.min_throughput_documents_per_second {
        return Err(PerformanceError::Throughput {
            observed: report.throughput_documents_per_second,
            allowed: baseline.min_throughput_documents_per_second,
        });
    }
    if report.p95_microseconds > baseline.max_p95_microseconds {
        return Err(PerformanceError::P95 {
            observed: report.p95_microseconds,
            allowed: baseline.max_p95_microseconds,
        });
    }
    if report.max_rss_kib > baseline.max_rss_kib {
        return Err(PerformanceError::Memory {
            observed: report.max_rss_kib,
            allowed: baseline.max_rss_kib,
        });
    }
    Ok(())
}

fn percentile(samples: &[u128], percentile: usize) -> u128 {
    let index = samples.len().saturating_sub(1) * percentile / 100;
    samples.get(index).copied().unwrap_or_default()
}
