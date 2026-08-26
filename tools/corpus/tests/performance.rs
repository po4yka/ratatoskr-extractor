//! Performance baseline threshold behavior.

use extractor_corpus::performance::{
    PerformanceBaseline, PerformanceError, PerformanceReport, check,
};

#[test]
fn baseline_check_rejects_measurements_outside_limits() {
    let baseline = PerformanceBaseline {
        report: report(10.0, 100, 786_432),
        min_throughput_documents_per_second: 10.0,
        max_p95_microseconds: 100,
        max_rss_kib: 786_432,
    };
    assert!(matches!(
        check(&report(9.0, 100, 786_432), &baseline),
        Err(PerformanceError::Throughput { .. })
    ));
    assert!(matches!(
        check(&report(10.0, 101, 786_432), &baseline),
        Err(PerformanceError::P95 { .. })
    ));
    assert!(matches!(
        check(&report(10.0, 100, 786_433), &baseline),
        Err(PerformanceError::Memory { .. })
    ));
}

fn report(
    throughput_documents_per_second: f64,
    p95_microseconds: u128,
    max_rss_kib: u64,
) -> PerformanceReport {
    PerformanceReport {
        case_count: 5,
        iterations: 1,
        throughput_documents_per_second,
        p50_microseconds: p95_microseconds,
        p95_microseconds,
        max_rss_kib,
    }
}
