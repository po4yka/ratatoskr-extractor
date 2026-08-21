//! Bounded shutdown contract tests.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use extractor_service::{AdmissionController, RuntimeHealth, ShutdownCoordinator, admin_router};
use tower::ServiceExt as _;

#[tokio::test(start_paused = true)]
async fn shutdown_refuses_new_work_before_the_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let health = RuntimeHealth::new();
    health.mark_ready();
    let admission = AdmissionController::new();
    let permit = admission.try_admit()?;
    let cancellation = permit.cancellation_token();
    let coordinator =
        ShutdownCoordinator::new(health.clone(), admission.clone(), Duration::from_secs(10));

    let shutdown = tokio::spawn(async move { coordinator.shutdown().await });
    tokio::task::yield_now().await;

    let response = admin_router(health, String::new)
        .oneshot(Request::get("/health/ready").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(admission.try_admit().is_err());
    assert!(!cancellation.is_cancelled());

    tokio::time::advance(Duration::from_secs(9)).await;
    assert!(!cancellation.is_cancelled());
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert!(cancellation.is_cancelled());

    drop(permit);
    shutdown.await?;
    Ok(())
}
