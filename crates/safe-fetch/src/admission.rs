use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::SafeFetchError;

#[derive(Debug, Clone)]
pub(super) struct Admission {
    global_limit: usize,
    per_host_limit: usize,
    state: Arc<Mutex<AdmissionState>>,
}

#[derive(Debug)]
struct AdmissionState {
    global: usize,
    per_host: HashMap<String, usize>,
}

#[derive(Debug)]
pub(super) struct AdmissionGuard {
    state: Arc<Mutex<AdmissionState>>,
    host: String,
}

impl Admission {
    pub(super) fn new(global_limit: usize, per_host_limit: usize) -> Self {
        Self {
            global_limit,
            per_host_limit,
            state: Arc::new(Mutex::new(AdmissionState {
                global: 0,
                per_host: HashMap::new(),
            })),
        }
    }

    pub(super) fn try_enter(&self, host: &str) -> Result<AdmissionGuard, SafeFetchError> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
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
        Ok(AdmissionGuard {
            state: Arc::clone(&self.state),
            host: host.to_owned(),
        })
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
