use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::SafeFetchError;

#[derive(Debug, Clone)]
pub(super) struct Admission {
    global_limit: usize,
    per_host_limit: usize,
    min_interval: Duration,
    state: Arc<Mutex<AdmissionState>>,
}

#[derive(Debug)]
struct AdmissionState {
    global: usize,
    per_host: HashMap<String, usize>,
    next_allowed: HashMap<String, Instant>,
}

#[derive(Debug)]
pub(super) struct AdmissionGuard {
    state: Arc<Mutex<AdmissionState>>,
    host: String,
}

impl Admission {
    pub(super) fn new(global_limit: usize, per_host_limit: usize, min_interval: Duration) -> Self {
        Self {
            global_limit,
            per_host_limit,
            min_interval,
            state: Arc::new(Mutex::new(AdmissionState {
                global: 0,
                per_host: HashMap::new(),
                next_allowed: HashMap::new(),
            })),
        }
    }

    /// Reserves one paced slot for `host` and admits it under the concurrency limits.
    ///
    /// The reservation advances before permit accounting so spacing holds even when permits free
    /// up out of order. Returns the instant the request may start; the caller sleeps until then,
    /// bounded by its own absolute deadline.
    pub(super) fn try_enter(
        &self,
        host: &str,
    ) -> Result<(AdmissionGuard, Instant), SafeFetchError> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        let start_at = if self.min_interval.is_zero() {
            now
        } else {
            let slot = state.next_allowed.get(host).copied().unwrap_or(now);
            let reserved = slot.max(now);
            state
                .next_allowed
                .insert(host.to_owned(), reserved + self.min_interval);
            reserved
        };
        let host_count = match state.per_host.get(host) {
            Some(count) => *count,
            None => 0,
        };
        if state.global >= self.global_limit || host_count >= self.per_host_limit {
            return Err(SafeFetchError::Overloaded);
        }
        state.global = state.global.saturating_add(1);
        state
            .per_host
            .insert(host.to_owned(), host_count.saturating_add(1));
        drop(state);
        Ok((
            AdmissionGuard {
                state: Arc::clone(&self.state),
                host: host.to_owned(),
            },
            start_at,
        ))
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.global = state.global.saturating_sub(1);
        let remove = if let Some(count) = state.per_host.get_mut(&self.host) {
            *count = count.saturating_sub(1);
            *count == 0
        } else {
            false
        };
        if remove {
            state.per_host.remove(&self.host);
        }
    }
}
