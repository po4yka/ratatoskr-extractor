//! Configuration contract tests.

use std::path::Path;

use extractor_core::{ConfigError, ExtractorConfig, load, load_from};
use figment::Figment;
use figment::Jail;
use figment::providers::Serialized;
use secrecy::ExposeSecret as _;
use serde_json::json;

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
    assert!(config.database.url.expose_secret().is_empty());
    assert!(config.database.max_connections > 0);
    assert!(config.bus.poll_interval_ms > 0);
    assert!(config.bus.worker_lease_seconds > 0);
    assert!(config.parser.max_input_bytes > 0);
    assert!(config.parser.max_dom_nodes > 0);
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

#[test]
fn pdf_defaults_are_bounded_and_overridable() -> Result<(), Box<dyn std::error::Error>> {
    let config = ExtractorConfig::built_in(Path::new("/var/lib/ratatoskr-extractor/blobs"));
    assert_eq!(config.pdf.max_input_bytes, 50 * 1_024 * 1_024);
    assert_eq!(config.pdf.max_pages, 1_000);
    assert_eq!(config.pdf.max_text_bytes, 8 * 1_024 * 1_024);

    let base = Figment::from(Serialized::defaults(ExtractorConfig::built_in(Path::new(
        "/var/lib/ratatoskr-extractor/blobs",
    ))))
    .merge((
        "database.url",
        "postgres://extractor:extractor@127.0.0.1:5434/extractor",
    ));
    let overridden = load_from(&base.merge(("pdf.max_pages", 7_u64)))?;
    assert_eq!(overridden.pdf.max_pages, 7);
    Ok(())
}

#[test]
fn provider_defaults_are_bounded_and_overridable() -> Result<(), Box<dyn std::error::Error>> {
    let config = ExtractorConfig::built_in(Path::new("/var/lib/ratatoskr-extractor/blobs"));
    assert_eq!(config.providers.max_input_bytes, 8 * 1_024 * 1_024);
    assert_eq!(config.providers.max_blocks, 2_000);

    let base = Figment::from(Serialized::defaults(ExtractorConfig::built_in(Path::new(
        "/var/lib/ratatoskr-extractor/blobs",
    ))))
    .merge((
        "database.url",
        "postgres://extractor:extractor@127.0.0.1:5434/extractor",
    ));
    let overridden = load_from(&base.merge(("providers.max_blocks", 11_u64)))?;
    assert_eq!(overridden.providers.max_blocks, 11);
    Ok(())
}

#[test]
fn rendering_is_off_by_default_and_budgets_parse() -> Result<(), Box<dyn std::error::Error>> {
    let config = ExtractorConfig::built_in(Path::new("/var/lib/ratatoskr-extractor/blobs"));
    assert!(
        !config.render.enabled,
        "escalation must stay inert by default"
    );
    assert_eq!(config.render.navigation_timeout_ms, 15_000);
    assert_eq!(config.render.total_timeout_ms, 45_000);

    let base = Figment::from(Serialized::defaults(ExtractorConfig::built_in(Path::new(
        "/var/lib/ratatoskr-extractor/blobs",
    ))))
    .merge((
        "database.url",
        "postgres://extractor:extractor@127.0.0.1:5434/extractor",
    ));
    let overridden = load_from(&base.merge(("render.enabled", true)))?;
    assert!(overridden.render.enabled);
    Ok(())
}

fn render_violation_report(
    overrides: &[(&str, serde_json::Value)],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut figment = Figment::from(Serialized::defaults(ExtractorConfig::built_in(Path::new(
        "/var/lib/ratatoskr-extractor/blobs",
    ))))
    .merge((
        "database.url",
        "postgres://extractor:extractor@127.0.0.1:5434/extractor",
    ));
    for (key, value) in overrides {
        figment = figment.merge((*key, value.clone()));
    }
    match load_from(&figment) {
        Ok(_) => Err("render configuration must be rejected".into()),
        Err(error @ ConfigError::Invalid(_)) => Ok(error.report()),
        Err(error) => Err(Box::new(error)),
    }
}

#[test]
fn render_policy_fields_validate_and_default_safe() -> Result<(), Box<dyn std::error::Error>> {
    let config = ExtractorConfig::built_in(Path::new("/var/lib/ratatoskr-extractor/blobs"));
    assert!(
        config.render.allowed_hosts.is_empty(),
        "an empty allowlist must impose no host restriction by default"
    );
    assert!(
        config.render.max_escalations_per_day > 0,
        "the daily escalation budget must be finite but reachable by default"
    );
    assert!(config.render.navigation_timeout_ms > 0);
    assert!(config.render.total_timeout_ms > 0);
    assert!(config.render.max_dom_bytes > 0);

    let base = Figment::from(Serialized::defaults(ExtractorConfig::built_in(Path::new(
        "/var/lib/ratatoskr-extractor/blobs",
    ))))
    .merge((
        "database.url",
        "postgres://extractor:extractor@127.0.0.1:5434/extractor",
    ));
    let overridden = load_from(&base.clone().merge((
        "render.allowed_hosts",
        vec!["example.com".to_owned(), "news.example.org".to_owned()],
    )))?;
    assert_eq!(
        overridden.render.allowed_hosts,
        ["example.com", "news.example.org"]
    );
    let capped = load_from(
        &base
            .clone()
            .merge(("render.max_escalations_per_day", 7_u64)),
    )?;
    assert_eq!(capped.render.max_escalations_per_day, 7);

    for (overrides, key, rule) in [
        (
            vec![
                ("render.enabled", json!(true)),
                ("render.max_escalations_per_day", json!(0)),
            ],
            "render.max_escalations_per_day",
            "must be greater than zero when rendering is enabled",
        ),
        (
            vec![("render.navigation_timeout_ms", json!(0))],
            "render.navigation_timeout_ms",
            "must be greater than zero",
        ),
        (
            vec![("render.total_timeout_ms", json!(0))],
            "render.total_timeout_ms",
            "must be greater than zero",
        ),
        (
            vec![("render.max_dom_bytes", json!(0))],
            "render.max_dom_bytes",
            "must be greater than zero",
        ),
    ] {
        let report = render_violation_report(&overrides)?;
        assert!(report.contains(key), "report must name {key}:\n{report}");
        assert!(
            report.contains(rule),
            "report must state the rule for {key}:\n{report}"
        );
    }
    Ok(())
}

#[test]
fn youtube_defaults_bound_transcripts_and_gate_media() {
    let config = ExtractorConfig::built_in(Path::new("/var/lib/ratatoskr-extractor/blobs"));
    assert_eq!(config.youtube.transcript.languages, ["en"]);
    assert!(
        !config.youtube.media.enabled,
        "media archival must stay off by default"
    );
    assert_eq!(
        config.youtube.media.max_item_bytes,
        2 * 1_024 * 1_024 * 1_024
    );
    assert_eq!(
        config.youtube.media.total_budget_bytes,
        8 * 1_024 * 1_024 * 1_024
    );
    assert_eq!(config.youtube.media.retention_hours, 24);
    assert_eq!(config.youtube.media.timeout_secs, 900);
    assert_eq!(config.youtube.media.max_height, 1080);
    assert_eq!(config.youtube.media.binary_path, "yt-dlp");
}

#[test]
#[allow(
    clippy::result_large_err,
    reason = "Figment Jail fixes the callback error type to figment::Error"
)]
fn youtube_environment_overrides_reach_transcripts_and_media() -> Result<(), figment::Error> {
    Jail::try_with(|jail| {
        jail.set_env(
            "RATATOSKR__BLOBS__ROOT",
            "/tmp/ratatoskr-youtube-test/blobs",
        );
        jail.set_env(
            "RATATOSKR__DATABASE__URL",
            "postgres://extractor:extractor@127.0.0.1:5434/extractor",
        );
        jail.set_env("RATATOSKR__YOUTUBE__TRANSCRIPT__LANGUAGES", "[ru, en]");
        jail.set_env("RATATOSKR__YOUTUBE__MEDIA__MAX_ITEM_BYTES", "123");

        let config = match load() {
            Ok(config) => config,
            Err(error) => return Err(error.to_string().into()),
        };
        assert_eq!(config.youtube.transcript.languages, ["ru", "en"]);
        assert_eq!(config.youtube.media.max_item_bytes, 123);
        Ok(())
    })?;
    Ok(())
}

fn youtube_violation_report(
    overrides: &[(&str, serde_json::Value)],
) -> Result<String, Box<dyn std::error::Error>> {
    let mut figment = Figment::from(Serialized::defaults(ExtractorConfig::built_in(Path::new(
        "/var/lib/ratatoskr-extractor/blobs",
    ))))
    .merge((
        "database.url",
        "postgres://extractor:extractor@127.0.0.1:5434/extractor",
    ));
    for (key, value) in overrides {
        figment = figment.merge((*key, value.clone()));
    }
    match load_from(&figment) {
        Ok(_) => Err("youtube configuration must be rejected".into()),
        Err(error @ ConfigError::Invalid(_)) => Ok(error.report()),
        Err(error) => Err(Box::new(error)),
    }
}

#[test]
fn invalid_youtube_settings_are_reported_without_their_values()
-> Result<(), Box<dyn std::error::Error>> {
    for (overrides, key, rule) in [
        (
            vec![("youtube.transcript.languages", json!([]))],
            "youtube.transcript.languages",
            "must contain at least one language code",
        ),
        (
            vec![("youtube.media.max_item_bytes", json!(0))],
            "youtube.media.max_item_bytes",
            "must be greater than zero",
        ),
        (
            vec![("youtube.media.total_budget_bytes", json!(0))],
            "youtube.media.total_budget_bytes",
            "must be greater than zero",
        ),
        (
            vec![
                ("youtube.media.max_item_bytes", json!(200)),
                ("youtube.media.total_budget_bytes", json!(100)),
            ],
            "youtube.media.total_budget_bytes",
            "must not be smaller than the per-item limit",
        ),
        (
            vec![("youtube.media.retention_hours", json!(0))],
            "youtube.media.retention_hours",
            "must be greater than zero",
        ),
        (
            vec![("youtube.media.timeout_secs", json!(0))],
            "youtube.media.timeout_secs",
            "must be greater than zero",
        ),
    ] {
        let report = youtube_violation_report(&overrides)?;
        assert!(report.contains(key), "report must name {key}:\n{report}");
        assert!(
            report.contains(rule),
            "report must state the rule for {key}:\n{report}"
        );
    }

    let budget_report = youtube_violation_report(&[
        ("youtube.media.max_item_bytes", json!(200)),
        ("youtube.media.total_budget_bytes", json!(100)),
    ])?;
    assert!(
        !budget_report.contains("200"),
        "per-item value leaked into the report:\n{budget_report}"
    );
    assert!(
        !budget_report.contains("100"),
        "budget value leaked into the report:\n{budget_report}"
    );
    Ok(())
}
