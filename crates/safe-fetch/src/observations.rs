use std::time::Duration;

pub(super) fn record_success(
    cache: &'static str,
    duration: Duration,
    wire_bytes: u64,
    decoded_bytes: u64,
) {
    metrics::counter!("ratatoskr_fetch_results_total", "cache" => cache).increment(1);
    metrics::histogram!("ratatoskr_extractor_fetch_duration_seconds")
        .record(duration.as_secs_f64());
    metrics::counter!("ratatoskr_extractor_fetch_bytes_total", "representation" => "wire")
        .increment(wire_bytes);
    metrics::counter!("ratatoskr_extractor_fetch_bytes_total", "representation" => "decoded")
        .increment(decoded_bytes);
}
