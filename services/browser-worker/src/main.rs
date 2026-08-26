//! Process entry point for the isolated Chromium rendering deployable.

use browser_worker::{ChromiumExecutor, WorkerSettings};

/// Launches Chromium with the default production navigation policy.
async fn production_executor(
    chrome_bin: Option<String>,
) -> Result<ChromiumExecutor, browser_worker::ExecutorError> {
    ChromiumExecutor::launch(chrome_bin).await
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
    let executor = match production_executor(settings.chrome_bin.clone()).await {
        Ok(executor) => executor,
        Err(error) => {
            eprintln!("Chromium launch failed: {error}");
            std::process::exit(1);
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
    if let Err(error) = Box::pin(browser_worker::run_render_consumer(
        context,
        settings.clone(),
        executor,
        cancellation.clone(),
    ))
    .await
    {
        eprintln!("render consumer failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(
        clippy::disallowed_methods,
        reason = "test-only Chrome location is not production configuration"
    )]
    fn chrome_bin() -> String {
        match std::env::var("CHROME_BIN") {
            Ok(path) => path,
            Err(_) => "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_owned(),
        }
    }

    fn loopback_command() -> render_job::RenderCommand {
        render_job::RenderCommand {
            render_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000d1"),
            operation_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000d2"),
            correlation_id: "operation:production-executor".to_owned(),
            tenant_user_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000d3"),
            url: "http://127.0.0.1:8080/fixture".to_owned(),
            budgets: render_job::RenderBudgets {
                navigation_timeout_ms: 1_000,
                total_timeout_ms: 2_000,
                max_dom_bytes: 16_384,
            },
        }
    }

    #[tokio::test]
    async fn production_executor_rejects_loopback_as_policy_blocked() {
        let executor = production_executor(Some(chrome_bin()))
            .await
            .expect("the test environment must provide Chrome or Chromium");
        let error = executor
            .render(&loopback_command())
            .await
            .expect_err("a loopback target must not render");
        assert!(
            matches!(error, browser_worker::ExecutorError::PolicyBlocked),
            "the production executor must enforce SSRF policy, got {error:?}"
        );
    }
}
