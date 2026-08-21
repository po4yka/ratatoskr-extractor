//! Admin-plane lifecycle tests.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode, header};
use extractor_service::{RuntimeHealth, admin_router};
use tower::ServiceExt as _;

#[tokio::test]
async fn readiness_follows_startup_and_drain() -> Result<(), Box<dyn std::error::Error>> {
    let health = RuntimeHealth::new();
    let router = admin_router(health.clone(), || "fetch_total 0\n".to_owned());

    assert_status(&router, "/health/live", StatusCode::OK).await?;
    assert_status(&router, "/health/ready", StatusCode::SERVICE_UNAVAILABLE).await?;

    health.mark_ready();
    assert_status(&router, "/health/ready", StatusCode::OK).await?;

    health.begin_drain();
    assert_status(&router, "/health/live", StatusCode::OK).await?;
    assert_status(&router, "/health/ready", StatusCode::SERVICE_UNAVAILABLE).await?;
    assert_status(&router, "/metrics", StatusCode::OK).await?;
    assert_status(&router, "/version", StatusCode::OK).await?;
    Ok(())
}

async fn assert_status(
    router: &Router,
    path: &str,
    expected: StatusCode,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = router
        .clone()
        .oneshot(Request::get(path).body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), expected);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL),
        Some(&HeaderValue::from_static("no-store"))
    );
    Ok(())
}
