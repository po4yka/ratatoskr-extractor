use std::time::{Duration, SystemTime};

use crate::SafeFetchError;

pub(super) const fn eligible(error: &SafeFetchError) -> bool {
    matches!(error, SafeFetchError::Transport)
}

pub(super) const fn transient_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
}

pub(super) fn retry_after_at(
    headers: &reqwest::header::HeaderMap,
    now: SystemTime,
) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value).ok().map(|target| {
        target
            .duration_since(now)
            .map_or(Duration::ZERO, |delay| delay)
    })
}

pub(super) fn delay(retry_index: u16, seed: u32, retry_after: Option<Duration>) -> Duration {
    let shift = u32::from(retry_index.min(6));
    let ceiling_ms = 100_u64.saturating_mul(1_u64 << shift);
    let mixed = u64::from(seed)
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    let jitter = Duration::from_millis(mixed % ceiling_ms.saturating_add(1));
    retry_after.map_or(jitter, |minimum| minimum.max(jitter))
}

pub(super) async fn sleep_before_deadline(
    deadline: tokio::time::Instant,
    delay: Duration,
) -> Result<(), SafeFetchError> {
    tokio::time::timeout_at(deadline, tokio::time::sleep(delay))
        .await
        .map_err(|_| SafeFetchError::TimeoutTotal)
}
