#![forbid(unsafe_code)]

//! Ratatoskr Extractor process entry point.

use std::path::Path;
use std::time::Duration;

use extractor_blob_store::BlobStore;
use extractor_core::{
    ExtractorConfig, ParserConfig, PdfConfig, ProvidersConfig, RenderConfig, YoutubeConfig,
};
use extractor_eventing::{NatsPublisher, claim_queued_run, run_command_consumer, run_outbox_once};
use extractor_persistence::Database;
use extractor_safe_fetch::SafeFetcher;
use extractor_service::{
    AdmissionController, ProcessError, RuntimeHealth, ShutdownCoordinator, admin_router,
    process_run,
};
use secrecy::ExposeSecret as _;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

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
    let result = run_initialized(&config, &telemetry).await;
    telemetry.shutdown();
    result
}

async fn run_initialized(
    config: &ExtractorConfig,
    telemetry: &extractor_telemetry::TelemetryGuard,
) -> Result<(), ProcessError> {
    let store = BlobStore::new(Path::new(&config.blobs.root));
    store.prepare().await?;
    let fetcher = SafeFetcher::new(config.fetch.clone(), &config.blobs.root)?;
    let database = Database::connect(
        config.database.url.expose_secret(),
        config.database.max_connections,
        Duration::from_millis(config.database.acquire_timeout_ms),
    )
    .await?;
    database.apply_schema().await?;
    let publisher = match &config.bus.nkey_seed_path {
        Some(path) => NatsPublisher::connect_with_nkey(&config.bus.url, path).await?,
        None => NatsPublisher::connect(&config.bus.url).await?,
    };
    publisher.ensure_command_stream().await?;
    publisher.ensure_event_stream().await?;
    let listener = tokio::net::TcpListener::bind(config.admin.bind).await?;
    let health = RuntimeHealth::new();
    let admission = AdmissionController::new();
    let shutdown = ShutdownCoordinator::new(
        health.clone(),
        admission.clone(),
        Duration::from_secs(config.shutdown.grace_seconds),
    );
    let metrics = telemetry.metrics_handle();
    let router = admin_router(health.clone(), move || metrics.render());
    let background_cancel = CancellationToken::new();
    let server_cancel = CancellationToken::new();
    let mut tasks = JoinSet::new();
    spawn_background_loops(
        &mut tasks,
        config,
        &database,
        &publisher,
        fetcher,
        store,
        health.clone(),
        admission.clone(),
        &background_cancel,
    );
    tasks.spawn({
        let cancellation = server_cancel.child_token();
        async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(cancellation.cancelled_owned())
                .await?;
            Ok::<_, ProcessError>(())
        }
    });
    health.mark_ready();

    let first_exit = tokio::select! {
        () = wait_for_signal() => None,
        task = tasks.join_next() => task,
    };
    background_cancel.cancel();
    shutdown.shutdown().await;
    server_cancel.cancel();
    let first_result = match first_exit {
        Some(result) => Some(result?),
        None => None,
    };
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    database.close().await;
    match first_result {
        Some(Ok(())) => Err(ProcessError::BackgroundStopped),
        Some(Err(error)) => Err(error),
        None => Ok(()),
    }
}

/// Spawns the four long-lived loops: command consumer, outbox, dependency probe, and worker.
#[allow(
    clippy::too_many_arguments,
    reason = "one handle per process resource, mirroring run_initialized's ownership split"
)]
fn spawn_background_loops(
    tasks: &mut JoinSet<Result<(), ProcessError>>,
    config: &ExtractorConfig,
    database: &Database,
    publisher: &NatsPublisher,
    fetcher: SafeFetcher,
    store: BlobStore,
    health: RuntimeHealth,
    admission: AdmissionController,
    background_cancel: &CancellationToken,
) {
    let bus = publisher.context().clone();
    tasks.spawn({
        let publisher = publisher.clone();
        let pool = database.pool().clone();
        let durable = config.bus.durable_name.clone();
        let cancellation = background_cancel.child_token();
        async move {
            run_command_consumer(&publisher, &pool, &durable, cancellation).await?;
            Ok::<_, ProcessError>(())
        }
    });
    tasks.spawn(outbox_loop(
        database.pool().clone(),
        publisher.clone(),
        config.bus.poll_interval_ms,
        config.bus.outbox_batch_size,
        background_cancel.child_token(),
    ));
    tasks.spawn(dependency_loop(
        database.pool().clone(),
        publisher.clone(),
        health,
        config.bus.poll_interval_ms,
        background_cancel.child_token(),
    ));
    tasks.spawn(worker_loop(
        database.pool().clone(),
        fetcher,
        store,
        config.parser.clone(),
        config.pdf.clone(),
        config.providers.clone(),
        config.render.clone(),
        config.youtube.clone(),
        bus,
        config.bus.worker_lease_seconds,
        config.bus.poll_interval_ms,
        admission,
        background_cancel.child_token(),
    ));
}

async fn outbox_loop(
    pool: sqlx::PgPool,
    publisher: NatsPublisher,
    poll_interval_ms: u64,
    batch_size: i64,
    cancellation: CancellationToken,
) -> Result<(), ProcessError> {
    let mut interval = tokio::time::interval(Duration::from_millis(poll_interval_ms));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok(()),
            _ = interval.tick() => {
                let report = run_outbox_once(&pool, &publisher, "extractor", batch_size).await
                    .map_err(extractor_persistence::PersistenceError::Query)?;
                tracing::debug!(claimed = report.claimed, published = report.published,
                    failed = report.failed, dead_lettered = report.dead_lettered,
                    "outbox pass completed");
            }
        }
    }
}

async fn dependency_loop(
    pool: sqlx::PgPool,
    publisher: NatsPublisher,
    health: RuntimeHealth,
    poll_interval_ms: u64,
    cancellation: CancellationToken,
) -> Result<(), ProcessError> {
    let mut interval = tokio::time::interval(Duration::from_millis(poll_interval_ms));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok(()),
            _ = interval.tick() => {
                let database_healthy = sqlx::query_scalar::<_, i32>("select 1")
                    .fetch_one(&pool)
                    .await
                    .is_ok();
                health.mark_dependencies_healthy(database_healthy && publisher.is_connected());
            }
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the worker loop owns one handle for each process resource and finite budget"
)]
async fn worker_loop(
    pool: sqlx::PgPool,
    fetcher: SafeFetcher,
    store: BlobStore,
    parser: ParserConfig,
    pdf: PdfConfig,
    providers: ProvidersConfig,
    render: RenderConfig,
    youtube: YoutubeConfig,
    bus: async_nats::jetstream::Context,
    lease_seconds: i32,
    poll_interval_ms: u64,
    admission: AdmissionController,
    cancellation: CancellationToken,
) -> Result<(), ProcessError> {
    let mut interval = tokio::time::interval(Duration::from_millis(poll_interval_ms));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok(()),
            _ = interval.tick() => {}
        }
        let Ok(permit) = admission.try_admit() else {
            return Ok(());
        };
        let Some(run) = claim_queued_run(&pool, "extractor", lease_seconds).await? else {
            drop(permit);
            continue;
        };
        let forced = permit.cancellation_token();
        tokio::select! {
            biased;
            () = forced.cancelled() => {}
            result = process_run(
                &pool,
                &fetcher,
                &store,
                &parser,
                &pdf,
                &providers,
                &render,
                &youtube,
                &bus,
                &run,
            ) => result?,
        }
        drop(permit);
    }
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
