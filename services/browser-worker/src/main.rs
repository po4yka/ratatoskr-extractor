//! Process entry point for the isolated Chromium rendering deployable.

use browser_worker::{RenderExecutor, RenderOutcome, WorkerError, WorkerSettings};

#[derive(Debug, Default)]
struct UnimplementedExecutor;

impl RenderExecutor for UnimplementedExecutor {
    async fn render(
        &self,
        _command: &render_job::RenderCommand,
    ) -> Result<RenderOutcome, WorkerError> {
        Err(WorkerError::Failed(
            render_job::RenderFailureClass::BrowserUnavailable,
        ))
    }
}

#[tokio::main]
async fn main() {
    let settings = match WorkerSettings::load() {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("browser worker configuration failed: {error}");
            std::process::exit(78);
        }
    };
    let cancellation = tokio_util::sync::CancellationToken::new();
    let client = match async_nats::connect(&settings.nats_url).await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("NATS connection failed: {error}");
            std::process::exit(1);
        }
    };
    let context = async_nats::jetstream::new(client);
    if let Err(error) = browser_worker::run_render_consumer(
        context,
        settings.clone(),
        UnimplementedExecutor,
        cancellation.clone(),
    )
    .await
    {
        eprintln!("render consumer failed: {error}");
        std::process::exit(1);
    }
}
