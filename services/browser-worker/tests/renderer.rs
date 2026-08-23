//! Integration tests against a real Chrome binary (`CHROME_BIN`).

use browser_worker::{ChromiumExecutor, ExecutorError};
use render_job::{RenderBudgets, RenderCommand};

fn budgets() -> RenderBudgets {
    RenderBudgets {
        navigation_timeout_ms: 15_000,
        total_timeout_ms: 45_000,
        max_dom_bytes: 65_536,
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only browser location is not process configuration"
)]
fn chrome_bin() -> String {
    match std::env::var("CHROME_BIN") {
        Ok(value) => value,
        // Local fallback for macOS developer machines; CI sets the variable explicitly.
        Err(_)
            if std::path::Path::new(
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            )
            .exists() =>
        {
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_owned()
        }
        Err(_) => String::new(),
    }
}

#[tokio::test]
async fn renders_a_page_under_budgets() -> Result<(), Box<dyn std::error::Error>> {
    let page_html = b"<html><body><h1>Rendered fixture</h1><img src=\"/img.png\"></body></html>";
    let server = extractor_test_support::ScriptedServer::start(vec![
        extractor_test_support::ScriptedResponse::chunks([bytes::Bytes::from_static(page_html)])
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("text/html"),
            ),
    ])
    .await?;
    let executor = ChromiumExecutor::launch_with_policy(
        Some(chrome_bin()),
        browser_worker::NavigationPolicy {
            routing: extractor_url_routing::RoutingPolicy {
                max_url_length: 8_192,
                allowed_ports: vec![80, 443, server.port()],
            },
            allow_loopback: true,
        },
    )
    .await?;
    let command = RenderCommand {
        render_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000ca"),
        operation_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000cb"),
        correlation_id: "operation:test".to_owned(),
        tenant_user_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000cc"),
        url: server.uri("/page"),
        budgets: budgets(),
    };

    let outcome = executor.render(&command).await?;
    let dom = String::from_utf8(outcome.dom.clone())?;
    assert!(
        dom.contains("Rendered fixture"),
        "the rendered DOM must contain the page marker"
    );
    assert_eq!(outcome.final_url, command.url);
    assert_eq!(outcome.evidence.blocked_requests, 1, "the image is denied");
    assert!(!outcome.evidence.hops.is_empty());
    Ok(())
}

#[tokio::test]
async fn oversized_dom_fails_as_size_limit() -> Result<(), Box<dyn std::error::Error>> {
    let big = format!("<html><body><p>{}</p></body></html>", "x".repeat(80_000));
    let server = extractor_test_support::ScriptedServer::start(vec![
        extractor_test_support::ScriptedResponse::chunks([bytes::Bytes::from(big)]).with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
    ])
    .await?;
    let executor = ChromiumExecutor::launch_with_policy(
        Some(chrome_bin()),
        browser_worker::NavigationPolicy {
            routing: extractor_url_routing::RoutingPolicy {
                max_url_length: 8_192,
                allowed_ports: vec![80, 443, server.port()],
            },
            allow_loopback: true,
        },
    )
    .await?;
    let command = RenderCommand {
        render_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000cd"),
        operation_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000ce"),
        correlation_id: "operation:test".to_owned(),
        tenant_user_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000cf"),
        url: server.uri("/big"),
        budgets: RenderBudgets {
            navigation_timeout_ms: 15_000,
            total_timeout_ms: 45_000,
            max_dom_bytes: 16_384,
        },
    };
    let outcome = executor.render(&command).await;
    match outcome {
        Err(ExecutorError::Budget(class)) => {
            assert_eq!(class.as_str(), "size_limit");
        }
        other => return Err(format!("expected size_limit failure, got {other:?}").into()),
    }
    Ok(())
}
