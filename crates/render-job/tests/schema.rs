//! Golden wire shapes pinned against the stored browser-rendering contract.

use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, MediaType,
};
use render_job::{
    NetworkEvidence, RedirectHop, RenderBudgets, RenderCommand, RenderCompleted, RenderFailed,
    RenderFailureClass,
};
use serde_json::json;

fn command() -> RenderCommand {
    RenderCommand {
        render_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000aa"),
        operation_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000ab"),
        correlation_id: "operation:018f0000-0000-7000-8000-0000000000ab".to_owned(),
        tenant_user_id: uuid::uuid!("018f0000-0000-7000-8000-0000000000ac"),
        url: "https://example.test/app".to_owned(),
        budgets: RenderBudgets {
            navigation_timeout_ms: 15_000,
            total_timeout_ms: 45_000,
            max_dom_bytes: 8 * 1_024 * 1_024,
        },
    }
}

#[test]
fn command_wire_shape_matches_the_contract() -> Result<(), serde_json::Error> {
    let encoded = serde_json::to_value(command())?;
    assert_eq!(
        encoded,
        json!({
            "render_id": "018f0000-0000-7000-8000-0000000000aa",
            "operation_id": "018f0000-0000-7000-8000-0000000000ab",
            "correlation_id": "operation:018f0000-0000-7000-8000-0000000000ab",
            "tenant_user_id": "018f0000-0000-7000-8000-0000000000ac",
            "url": "https://example.test/app",
            "budgets": {
                "navigation_timeout_ms": 15000,
                "total_timeout_ms": 45000,
                "max_dom_bytes": 8 * 1_024 * 1_024,
            },
        })
    );
    let decoded: RenderCommand = serde_json::from_value(encoded)?;
    assert_eq!(decoded, command());
    Ok(())
}

#[test]
fn unknown_command_fields_are_rejected() {
    let mut smuggled = serde_json::to_value(command()).expect("fixture serializes");
    smuggled["cookie"] = json!("session=value");
    let outcome = serde_json::from_value::<RenderCommand>(smuggled);
    assert!(
        outcome.is_err(),
        "the schema must not express credential fields"
    );
}

#[test]
fn completion_wire_shape_carries_owned_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let completed = RenderCompleted {
        render_id: command().render_id,
        final_url: "https://example.test/app".to_owned(),
        dom: worker_blob(11)?,
        evidence: NetworkEvidence {
            hops: vec![RedirectHop {
                url: "https://example.test/app".to_owned(),
                status: 200,
                media_type: Some("text/html".to_owned()),
            }],
            blocked_requests: 4,
        },
    };
    let encoded = serde_json::to_value(&completed)?;
    assert_eq!(
        encoded["dom"]["owner_service"],
        json!("ratatoskr-browser-worker")
    );
    assert_eq!(encoded["dom"]["media_type"], json!("text/html"));
    assert_eq!(encoded["evidence"]["blocked_requests"], json!(4));
    assert_eq!(encoded["evidence"]["hops"][0]["status"], json!(200));
    let decoded: RenderCompleted = serde_json::from_value(encoded)?;
    assert_eq!(decoded, completed);
    Ok(())
}

#[test]
fn failure_classes_keep_stable_wire_spellings() -> Result<(), serde_json::Error> {
    for (class, spelling) in [
        (RenderFailureClass::PolicyBlocked, "policy_blocked"),
        (RenderFailureClass::NavigationTimeout, "navigation_timeout"),
        (RenderFailureClass::TotalTimeout, "total_timeout"),
        (RenderFailureClass::SizeLimit, "size_limit"),
        (RenderFailureClass::NavigationFailed, "navigation_failed"),
        (
            RenderFailureClass::BrowserUnavailable,
            "browser_unavailable",
        ),
    ] {
        let failed = RenderFailed {
            render_id: command().render_id,
            class,
        };
        let encoded = serde_json::to_value(&failed)?;
        assert_eq!(encoded["class"], json!(spelling));
        let decoded: RenderFailed = serde_json::from_value(encoded)?;
        assert_eq!(decoded.class.as_str(), spelling);
    }
    Ok(())
}

fn worker_blob(length: usize) -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-browser-worker")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"77".repeat(32))?,
        },
        media_type: MediaType::parse("text/html")?,
        length_bytes: u64::try_from(length)?,
    })
}
