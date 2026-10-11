//! The shipped operator example is a configuration the worker actually loads (XR-021
//! CONTRACTS.md R2-10 rule 2).

use browser_worker::WorkerSettings;
use figment::Jail;

const EXAMPLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../deploy/systemd/browser-worker.conf.example"
);

/// The `KEY=value` lines of an `EnvironmentFile`, without comments and blank lines.
fn assignments(text: &str) -> Vec<(&str, &str)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .collect()
}

#[test]
#[allow(
    clippy::result_large_err,
    reason = "Figment Jail fixes the callback error type to figment::Error"
)]
fn the_operator_example_loads_through_the_worker_settings() {
    let example = std::fs::read_to_string(EXAMPLE);
    assert!(
        example.is_ok(),
        "the operator example must exist at {EXAMPLE}"
    );
    let example = example.unwrap_or_default();
    let assignments = assignments(&example);
    assert!(
        assignments
            .iter()
            .all(|(key, _)| *key != "BROWSER_PROVISION_TOPOLOGY"),
        "production topology comes from Edge, so the example never provisions"
    );
    Jail::expect_with(|jail| {
        // The seed path is the one value that must name a file on the machine running the test.
        jail.create_file("worker.nkey", "SUANOTAREALSEEDFORTHELOADINGTEST")?;
        let seed = jail.directory().join("worker.nkey");
        for (key, value) in &assignments {
            if *key == "BROWSER_NKEY_SEED_PATH" {
                jail.set_env(key, seed.to_string_lossy());
            } else {
                jail.set_env(key, value);
            }
        }

        let settings = WorkerSettings::load().map_err(figment::Error::from)?;

        assert_eq!(settings.nats_url, "nats://127.0.0.1:4222");
        assert_eq!(settings.nkey_seed_path.as_deref(), Some(seed.as_path()));
        assert_eq!(
            settings.blobs_root,
            std::path::Path::new("/mnt/nvme/ratatoskr/blobs/ratatoskr-extractor-browser-worker")
        );
        assert_eq!(settings.durable_name, "ratatoskr_browser_worker");
        assert_eq!(settings.completions_bucket, "browser_worker_completions");
        assert_eq!(settings.max_jobs_per_process, 500);
        assert!(!settings.provision_topology);
        Ok(())
    });
}
