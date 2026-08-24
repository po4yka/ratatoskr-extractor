//! Test-only constructors for [`SafeFetcher`], compiled under `test` or the `test-support`
//! feature and never shipped in the default build.
//!
//! Only the validating DNS resolver is omitted so tests may reach loopback; port, redirect,
//! size, and timeout policies stay enforced from the supplied configuration.

#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::pin::Pin;

use extractor_blob_store::BlobStore;
use extractor_core::FetchConfig;
use reqwest::redirect::Policy;

use crate::admission::Admission;
#[cfg(test)]
use crate::{FetchRequest, FetchResult};
use crate::{SafeFetchError, SafeFetcher};

impl SafeFetcher {
    /// Builds a fetcher for tests against local scripted servers.
    ///
    /// # Errors
    ///
    /// Returns [`SafeFetchError`] when the HTTP client cannot be constructed.
    pub fn new_for_test(config: FetchConfig, store: BlobStore) -> Result<Self, SafeFetchError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .user_agent("Ratatoskr-Extractor/0.1-test")
            .build()
            .map_err(SafeFetchError::Client)?;
        Ok(Self {
            client,
            store,
            admission: Admission::new(
                config.global_concurrency,
                config.per_host_concurrency,
                std::time::Duration::from_millis(config.per_host_min_interval_ms),
            ),
            resolver: None,
            config,
        })
    }

    /// Runs one fetch with the literal-address policy relaxed for loopback scripts.
    #[cfg(test)]
    pub(crate) fn fetch_for_test(
        &self,
        request: FetchRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FetchResult, SafeFetchError>> + Send + '_>> {
        Box::pin(self.fetch_inner(request, false))
    }
}
