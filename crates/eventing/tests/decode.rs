//! Capture command wire validation against the typed `content.capture.requested.v1` contract.

use extractor_eventing::{CaptureSource, ConsumeError, decode_capture};
use extractor_test_support::capture::CaptureCommandJson;
use serde_json::json;

const SUBJECT: &str = "cmd.content.capture.requested.v1";

/// `fixtures/commands/content.capture.requested.v1/valid/url.json` of ratatoskr-contracts.
fn contract_url_payload() -> serde_json::Value {
    json!({
        "operation_id": "018f0000-0000-7000-8000-000000000a11",
        "idempotency_key": {
            "algorithm": "sha256",
            "hex": "1f39abea4267b2f2e026c72f605e53c9f0dc62e104fe317bf0c0471f3346de2e"
        },
        "url": "https://example.test/articles/borrow-checker"
    })
}

#[test]
fn decodes_the_typed_content_capture_command() -> Result<(), Box<dyn std::error::Error>> {
    let operation_id = uuid::uuid!("018f0000-0000-7000-8000-000000000a11");
    let mut document = CaptureCommandJson::url("https://example.test/unused");
    document.operation_id = operation_id;
    document.aggregate_id = format!("operation:{operation_id}");
    document.payload = contract_url_payload();

    let command = decode_capture(SUBJECT, &document.to_bytes())?;

    assert_eq!(command.command_id, document.command_id);
    assert_eq!(command.operation_id.0, operation_id);
    assert_eq!(command.tenant_id.user_id().0, document.tenant_user);
    assert_eq!(command.correlation_id.to_string(), document.correlation_id);
    assert_eq!(
        command.idempotency_key,
        "1f39abea4267b2f2e026c72f605e53c9f0dc62e104fe317bf0c0471f3346de2e"
    );
    assert_eq!(command.requested_at.to_string(), "2026-08-21T10:00:00Z");
    Ok(())
}

#[test]
fn url_payload_still_decodes_to_a_url_source() -> Result<(), Box<dyn std::error::Error>> {
    let document = CaptureCommandJson::url("https://example.test/article");
    let command = decode_capture(SUBJECT, &document.to_bytes())?;
    match command.source {
        CaptureSource::Url(url) => assert_eq!(url.as_str(), "https://example.test/article"),
        CaptureSource::Blob(_) => return Err("a url payload must decode to a url source".into()),
    }
    Ok(())
}

#[test]
fn blob_payload_decodes_to_a_blob_source() -> Result<(), Box<dyn std::error::Error>> {
    let digest = "33d13663b80c35b99fe73e7ee27db248affa1fe18c0b1d5b7237c98f75425726";
    let document =
        CaptureCommandJson::blob("ratatoskr-telegram", digest, "application/pdf", 13_264);
    let command = decode_capture(SUBJECT, &document.to_bytes())?;
    match command.source {
        CaptureSource::Blob(blob) => {
            assert_eq!(blob.owner_service.as_str(), "ratatoskr-telegram");
            assert_eq!(blob.digest.hex.as_str(), digest);
            assert_eq!(blob.media_type.as_str(), "application/pdf");
            assert_eq!(blob.length_bytes, 13_264);
        }
        CaptureSource::Url(_) => return Err("a blob payload must decode to a blob source".into()),
    }
    Ok(())
}

#[test]
fn payload_with_both_url_and_blob_is_rejected() {
    let mut document = CaptureCommandJson::blob(
        "ratatoskr-telegram",
        "33d13663b80c35b99fe73e7ee27db248affa1fe18c0b1d5b7237c98f75425726",
        "application/pdf",
        13_264,
    );
    if let Some(members) = document.payload.as_object_mut() {
        members.insert("url".to_owned(), json!("https://example.test/article"));
    }
    assert!(decode_capture(SUBJECT, &document.to_bytes()).is_err());
}

#[test]
fn an_aggregate_id_that_disagrees_with_the_payload_operation_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let mut document = CaptureCommandJson::url("https://example.test/article");
    document.aggregate_id = format!("operation:{}", uuid::Uuid::now_v7());
    let error = decode_capture(SUBJECT, &document.to_bytes())
        .err()
        .ok_or("a disagreeing aggregate must be refused")?;
    assert!(
        matches!(error, ConsumeError::AggregateMismatch),
        "{error:?}"
    );
    Ok(())
}

#[test]
fn envelopes_that_break_the_contract_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let base = CaptureCommandJson::url("https://example.test/article");

    let mut wrong_type = base.to_value();
    wrong_type["command_type"] = json!("content.capture.wrong.v1");
    let mut bad_time = base.to_value();
    bad_time["issued_at"] = json!("not-an-instant");
    let mut bad_tenant = base.to_value();
    bad_tenant["tenant_id"] = json!(format!("admin:{}", base.tenant_user));
    let mut no_tenant = base.to_value();
    if let Some(members) = no_tenant.as_object_mut() {
        members.remove("tenant_id");
    }
    let mut foreign_producer = base.to_value();
    foreign_producer["producer"] = json!("ratatoskr-knowledge");
    let mut file_url = base.to_value();
    file_url["payload"]["url"] = json!("file:///etc/passwd");
    let legacy = json!({
        "command_id": base.command_id,
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-21T10:00:00Z",
        "operation_id": base.operation_id,
        "tenant_id": format!("user:{}", base.tenant_user),
        "correlation_id": base.correlation_id,
        "idempotency_key": "legacy",
        "payload": { "url": "https://example.test/article" }
    });

    for invalid in [
        wrong_type,
        bad_time,
        bad_tenant,
        no_tenant,
        foreign_producer,
        file_url,
        legacy,
    ] {
        assert!(
            decode_capture(SUBJECT, &serde_json::to_vec(&invalid)?).is_err(),
            "{invalid}"
        );
    }
    assert!(decode_capture("cmd.content.other.v1", &base.to_bytes()).is_err());
    Ok(())
}
