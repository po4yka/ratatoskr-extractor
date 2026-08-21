//! Telemetry redaction contract tests.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use extractor_telemetry::{FetchFailureClass, record_fetch_failure};
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::{Layer, Registry};

#[derive(Debug, Clone, Default)]
struct CaptureLayer {
    records: Arc<Mutex<String>>,
}

#[derive(Debug, Default)]
struct FieldVisitor {
    record: String,
}

impl<S> Layer<S> for CaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        if let Ok(mut records) = self.records.lock() {
            records.push_str(event.metadata().name());
            records.push_str(&visitor.record);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.record, " {}={value:?}", field.name());
    }
}

#[test]
fn fetch_failure_telemetry_excludes_url_and_query_secret() -> Result<(), Box<dyn std::error::Error>>
{
    let target = "https://example.com/private/path?token=LEAKME";
    assert!(target.contains("LEAKME"));

    let capture = CaptureLayer::default();
    let records = Arc::clone(&capture.records);
    let subscriber = Registry::default().with(capture);
    tracing::subscriber::with_default(subscriber, || {
        record_fetch_failure(FetchFailureClass::PolicyDenied);
    });

    let output = match records.lock() {
        Ok(output) => output.clone(),
        Err(_) => return Err(std::io::Error::other("capture lock was poisoned").into()),
    };
    assert!(output.contains("fetch_failure"));
    assert!(output.contains("policy_denied"));
    for forbidden in [
        "LEAKME",
        "example.com",
        "/private",
        "token",
        "header",
        "body",
    ] {
        assert!(
            !output.contains(forbidden),
            "captured forbidden field {forbidden}"
        );
    }
    Ok(())
}
