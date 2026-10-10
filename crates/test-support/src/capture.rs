//! Typed `content.capture.requested.v1` command documents (XR-021 CONTRACTS.md S11).

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// A capture command in the contract wire shape: a `CommandEnvelope` whose payload names exactly
/// one source.
///
/// Every member is public so a test can break one invariant at a time, for example an aggregate
/// that disagrees with the payload operation.
#[derive(Debug, Clone)]
pub struct CaptureCommandJson {
    /// Delivery identity and inbox deduplication key.
    pub command_id: uuid::Uuid,
    /// Platform operation named by the payload and by the aggregate.
    pub operation_id: uuid::Uuid,
    /// Owner of the requested work, rendered as `user:<uuid>`.
    pub tenant_user: uuid::Uuid,
    /// Envelope `producer`.
    pub producer: String,
    /// Envelope `aggregate_id`.
    pub aggregate_id: String,
    /// Envelope `correlation_id`.
    pub correlation_id: String,
    /// Payload object, replaceable as a whole.
    pub payload: Value,
}

impl CaptureCommandJson {
    /// A command that asks for one public URL.
    #[must_use]
    pub fn url(url: &str) -> Self {
        Self::with_source("capture", json!({ "url": url }))
    }

    /// A command that names one stored blob.
    #[must_use]
    pub fn blob(owner: &str, digest_hex: &str, media_type: &str, length_bytes: u64) -> Self {
        Self::with_source(
            "capture-blob",
            json!({ "blob": {
                "owner_service": owner,
                "digest": { "algorithm": "sha256", "hex": digest_hex },
                "media_type": media_type,
                "length_bytes": length_bytes,
            }}),
        )
    }

    fn with_source(idempotency: &str, source: Value) -> Self {
        let operation_id = uuid::Uuid::now_v7();
        let mut payload = json!({
            "operation_id": operation_id,
            "idempotency_key": { "algorithm": "sha256", "hex": sha256_hex(idempotency) },
        });
        if let (Some(target), Value::Object(members)) = (payload.as_object_mut(), source) {
            target.extend(members);
        }
        Self {
            command_id: uuid::Uuid::now_v7(),
            operation_id,
            tenant_user: uuid::Uuid::now_v7(),
            producer: "ratatoskr-platform".to_owned(),
            aggregate_id: format!("operation:{operation_id}"),
            correlation_id: format!("operation:{operation_id}"),
            payload,
        }
    }

    /// Replaces the idempotency string behind the payload's `idempotency_key`.
    #[must_use]
    pub fn with_idempotency(mut self, idempotency: &str) -> Self {
        if let Some(members) = self.payload.as_object_mut() {
            members.insert(
                "idempotency_key".to_owned(),
                json!({ "algorithm": "sha256", "hex": sha256_hex(idempotency) }),
            );
        }
        self
    }

    /// The envelope as a JSON value.
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "command_id": self.command_id,
            "command_type": "content.capture.requested.v1",
            "issued_at": "2026-08-21T10:00:00Z",
            "producer": self.producer,
            "aggregate_id": self.aggregate_id,
            "correlation_id": self.correlation_id,
            "tenant_id": format!("user:{}", self.tenant_user),
            "schema_version": 1,
            "payload": self.payload,
        })
    }

    /// The envelope as the bytes a broker delivers.
    ///
    /// # Panics
    ///
    /// Never in practice: a `serde_json::Value` always serializes.
    #[must_use]
    #[expect(clippy::expect_used, reason = "a JSON value always serializes")]
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.to_value()).expect("a JSON value always serializes")
    }
}

/// Lowercase hexadecimal SHA-256 of a string, the shape of `payload.idempotency_key.hex`.
#[must_use]
pub fn sha256_hex(text: &str) -> String {
    use std::fmt::Write as _;

    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            let _ = write!(output, "{byte:02x}");
            output
        })
}
