//! Capture command wire validation.

use extractor_eventing::decode_capture;
use serde_json::json;

const SUBJECT: &str = "cmd.content.capture.requested.v1";

#[test]
fn platform_capture_wire_shape_is_safely_decoded() -> Result<(), Box<dyn std::error::Error>> {
    let operation_id = uuid::Uuid::now_v7();
    let command_id = uuid::Uuid::now_v7();
    let owner_id = uuid::Uuid::now_v7();
    let valid = json!({
        "command_id": command_id,
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-21T10:00:00Z",
        "operation_id": operation_id,
        "tenant_id": format!("user:{owner_id}"),
        "correlation_id": format!("operation:{operation_id}"),
        "idempotency_key": "capture-safe-decode",
        "payload": { "url": "https://example.test/article" },
        "future_envelope_member": true
    });
    assert!(decode_capture(SUBJECT, &serde_json::to_vec(&valid)?).is_ok());

    for invalid in [
        json!({
            "command_id": command_id,
            "command_type": "content.capture.wrong.v1",
            "requested_at": "2026-08-21T10:00:00Z",
            "operation_id": operation_id,
            "tenant_id": format!("user:{owner_id}"),
            "correlation_id": format!("operation:{operation_id}"),
            "idempotency_key": "capture-wrong-type",
            "payload": { "url": "https://example.test/article" }
        }),
        json!({
            "command_id": command_id,
            "command_type": "content.capture.requested.v1",
            "requested_at": "not-an-instant",
            "operation_id": operation_id,
            "tenant_id": format!("user:{owner_id}"),
            "correlation_id": format!("operation:{operation_id}"),
            "idempotency_key": "capture-wrong-time",
            "payload": { "url": "https://example.test/article" }
        }),
        json!({
            "command_id": command_id,
            "command_type": "content.capture.requested.v1",
            "requested_at": "2026-08-21T10:00:00Z",
            "operation_id": operation_id,
            "tenant_id": format!("admin:{owner_id}"),
            "correlation_id": format!("operation:{operation_id}"),
            "idempotency_key": "capture-wrong-owner",
            "payload": { "url": "https://example.test/article" }
        }),
        json!({
            "command_id": command_id,
            "command_type": "content.capture.requested.v1",
            "requested_at": "2026-08-21T10:00:00Z",
            "operation_id": operation_id,
            "tenant_id": format!("user:{owner_id}"),
            "correlation_id": format!("operation:{operation_id}"),
            "idempotency_key": "capture-file-url",
            "payload": { "url": "file:///etc/passwd" }
        }),
    ] {
        assert!(decode_capture(SUBJECT, &serde_json::to_vec(&invalid)?).is_err());
    }
    Ok(())
}
