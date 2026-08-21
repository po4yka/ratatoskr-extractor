//! Configuration contract tests.

use std::path::Path;

use extractor_core::{ExtractorConfig, load};
use figment::Jail;

#[test]
fn defaults_are_finite_and_security_cannot_be_disabled() -> Result<(), serde_json::Error> {
    let config = ExtractorConfig::built_in(Path::new("/var/lib/ratatoskr-extractor/blobs"));

    assert!(config.admin.bind.ip().is_loopback());
    assert_eq!(config.fetch.allowed_ports, [80, 443]);
    assert!(config.fetch.max_url_length > 0);
    assert!(config.fetch.connect_timeout_ms > 0);
    assert!(config.fetch.first_byte_timeout_ms > 0);
    assert!(config.fetch.read_idle_timeout_ms > 0);
    assert!(config.fetch.total_timeout_ms > 0);
    assert!(config.fetch.max_wire_bytes > 0);
    assert!(config.fetch.max_decoded_bytes > 0);
    assert!(config.fetch.max_redirects > 0);
    assert!(config.fetch.max_retries > 0);
    assert!(config.fetch.global_concurrency > 0);
    assert!(config.fetch.per_host_concurrency > 0);
    assert!(config.shutdown.grace_seconds > 0);

    let encoded = serde_json::to_string(&config)?;
    assert!(!encoded.contains("disable"));
    assert!(!encoded.contains("unsafe"));
    Ok(())
}

#[test]
#[allow(
    clippy::result_large_err,
    reason = "Figment Jail fixes the callback error type to figment::Error"
)]
fn invalid_environment_is_reported_without_its_value() -> Result<(), figment::Error> {
    for variable in [
        "RATATOSKR__FETCH__UNKNOWN_LIMIT",
        "RATATOSKR__FETCH__TOTAL_TIMEOUT_MS",
    ] {
        Jail::try_with(|jail| {
            jail.set_env(variable, "LEAKME");

            let report = match load() {
                Ok(_) => String::new(),
                Err(error) => error.report(),
            };
            assert!(report.contains(variable));
            assert!(!report.contains("LEAKME"));
            Ok(())
        })?;
    }
    Ok(())
}
