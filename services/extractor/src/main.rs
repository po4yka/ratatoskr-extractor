#![forbid(unsafe_code)]

//! Ratatoskr Extractor process entry point.

use std::path::Path;
use std::time::Duration;

use extractor_blob_store::BlobStore;
use extractor_core::ExtractorConfig;
use extractor_safe_fetch::SafeFetcher;
use extractor_service::{AdmissionController, RuntimeHealth, ShutdownCoordinator, admin_router};

#[tokio::main]
async fn main() {
    let config = match extractor_core::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{}", error.report());
            std::process::exit(error.exit_code().into());
        }
    };
    match command() {
        Ok(Command::CheckConfig) => {}
        Ok(Command::Run) => {
            if run(config).await.is_err() {
                eprintln!("extractor startup or runtime failed");
                std::process::exit(1);
            }
        }
        Err(()) => {
            eprintln!("usage: ratatoskr-extractor [check-config]");
            std::process::exit(64);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Command {
    Run,
    CheckConfig,
}

#[derive(Debug, thiserror::Error)]
enum ProcessError {
    #[error("artifact storage initialization failed")]
    Blob(#[from] extractor_blob_store::BlobStoreError),
    #[error("safe fetch initialization failed")]
    Fetch(#[from] extractor_safe_fetch::SafeFetchError),
    #[error("telemetry initialization failed")]
    Telemetry(#[from] extractor_telemetry::TelemetryError),
    #[error("admin listener failed")]
    Io(#[from] std::io::Error),
}

fn command() -> Result<Command, ()> {
    let mut arguments = std::env::args_os().skip(1);
    match (arguments.next(), arguments.next()) {
        (None, None) => Ok(Command::Run),
        (Some(value), None) if value == "check-config" => Ok(Command::CheckConfig),
        _ => Err(()),
    }
}

async fn run(config: ExtractorConfig) -> Result<(), ProcessError> {
    let telemetry = extractor_telemetry::init(&config.telemetry)?;
    let store = BlobStore::new(Path::new(&config.blobs.root));
    store.prepare().await?;
    let _fetcher = SafeFetcher::new(config.fetch.clone(), &config.blobs.root)?;
    let listener = tokio::net::TcpListener::bind(config.admin.bind).await?;
    let health = RuntimeHealth::new();
    let admission = AdmissionController::new();
    let shutdown = ShutdownCoordinator::new(
        health.clone(),
        admission,
        Duration::from_secs(config.shutdown.grace_seconds),
    );
    let metrics = telemetry.metrics_handle();
    let router = admin_router(health.clone(), move || metrics.render());
    health.mark_ready();
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(shutdown))
        .await?;
    telemetry.shutdown();
    Ok(())
}

async fn shutdown_signal(shutdown: ShutdownCoordinator) {
    wait_for_signal().await;
    shutdown.shutdown().await;
}

#[cfg(unix)]
async fn wait_for_signal() {
    let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
    match terminate {
        Ok(mut terminate) => {
            tokio::select! {
                result = tokio::signal::ctrl_c() => { let _ = result; }
                signal = terminate.recv() => { let _ = signal; }
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
