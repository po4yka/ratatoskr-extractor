//! Scripted local server contract tests.

use std::sync::Arc;

use bytes::Bytes;
use extractor_test_support::{ScriptedResponse, ScriptedServer};
use reqwest::redirect::Policy;
use tokio::sync::Notify;

#[tokio::test]
async fn scripted_server_records_requests_without_sleeping()
-> Result<(), Box<dyn std::error::Error>> {
    let gate = Arc::new(Notify::new());
    let server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([Bytes::from_static(b"alpha"), Bytes::from_static(b"beta")]),
        ScriptedResponse::redirect("/final"),
        ScriptedResponse::chunks([Bytes::from_static(b"released")]).stall_until(Arc::clone(&gate)),
    ])
    .await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .build()?;

    let first = client
        .get(server.uri("/chunks"))
        .header("x-test", "one")
        .send()
        .await?;
    assert_eq!(first.text().await?, "alphabeta");

    let redirect = client.get(server.uri("/redirect")).send().await?;
    assert_eq!(redirect.status(), reqwest::StatusCode::FOUND);
    assert_eq!(
        redirect.headers().get(reqwest::header::LOCATION),
        Some(&reqwest::header::HeaderValue::from_static("/final"))
    );

    let stalled_client = client.clone();
    let stalled_uri = server.uri("/stall");
    let stalled = tokio::spawn(async move { stalled_client.get(stalled_uri).send().await });
    server.wait_for_requests(3).await;
    assert_eq!(server.request_count(), 3);
    gate.notify_one();
    assert_eq!(stalled.await??.text().await?, "released");

    let requests = server.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .first()
            .and_then(|request| request.headers.get("x-test")),
        Some(&reqwest::header::HeaderValue::from_static("one"))
    );
    Ok(())
}
