//! `BROWSER_*` settings that select the bus identity and the provisioning switch.

use browser_worker::WorkerSettings;
use figment::Jail;

#[test]
#[allow(
    clippy::result_large_err,
    reason = "Figment Jail fixes the callback error type to figment::Error"
)]
fn nkey_seed_path_comes_from_the_environment() -> Result<(), Box<dyn std::error::Error>> {
    Jail::try_with(|jail| {
        let defaults = WorkerSettings::load().map_err(figment::Error::from)?;
        assert_eq!(defaults.nkey_seed_path, None);
        assert!(
            !defaults.provision_topology,
            "topology is never created by default"
        );

        jail.set_env(
            "BROWSER_NKEY_SEED_PATH",
            "/etc/ratatoskr/extractor-browser-worker.nkey",
        );
        let identified = WorkerSettings::load().map_err(figment::Error::from)?;
        assert_eq!(
            identified.nkey_seed_path.as_deref(),
            Some(std::path::Path::new(
                "/etc/ratatoskr/extractor-browser-worker.nkey"
            ))
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
#[allow(
    clippy::result_large_err,
    reason = "Figment Jail fixes the callback error type to figment::Error"
)]
fn provisioning_is_read_and_refused_together_with_a_seed() -> Result<(), Box<dyn std::error::Error>>
{
    Jail::try_with(|jail| {
        jail.set_env("BROWSER_PROVISION_TOPOLOGY", "true");
        let provisioning = WorkerSettings::load().map_err(figment::Error::from)?;
        assert!(provisioning.provision_topology);

        jail.set_env(
            "BROWSER_NKEY_SEED_PATH",
            "/etc/ratatoskr/extractor-browser-worker.nkey",
        );
        assert!(
            WorkerSettings::load().is_err(),
            "provisioning is for unauthenticated brokers and must not combine with a seed"
        );
        Ok(())
    })?;
    Ok(())
}
