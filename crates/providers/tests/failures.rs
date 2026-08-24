//! Typed provider failure modes: schema violations, degradation, and budgets.

use extractor_providers::{
    ProviderError, ProviderInput, ProviderLimits, SourceRoute, from_provider,
};
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn schema_violations_are_typed() -> Result<(), Box<dyn std::error::Error>> {
    let missing_title = br#"{"id": 900, "title": null, "text": null, "children": []}"#;
    let outcome = from_provider(
        test_input(SourceRoute::HackerNews, missing_title)?,
        limits(),
    );
    assert!(matches!(outcome, Err(ProviderError::Schema)));

    let without_post =
        br#"[{"kind":"Listing","data":{"children":[{"kind":"t1","data":{"body":"only a comment"}}]}}]"#;
    let outcome = from_provider(test_input(SourceRoute::Reddit, without_post)?, limits());
    assert!(matches!(outcome, Err(ProviderError::Schema)));

    let challenge = include_bytes!("fixtures/reddit-challenge.html");
    let outcome = from_provider(test_input(SourceRoute::Reddit, challenge)?, limits());
    assert!(matches!(outcome, Err(ProviderError::Schema)));
    Ok(())
}

#[test]
fn link_only_story_carries_external_url_past_the_gate() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/hn-minimal.json");
    let Ok(extraction) = from_provider(test_input(SourceRoute::HackerNews, bytes)?, limits())
    else {
        return Err("a link-only story must pass through with its article URL".into());
    };
    assert_eq!(
        extraction.external_url.as_deref(),
        Some("https://example.com/link-only"),
        "the canonical article URL is carried for resolution"
    );
    let candidates = &extraction.candidates;
    let candidate = candidates.first().ok_or("candidate evidence is attached")?;
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidate.strategy, "hacker_news_item");
    assert!(candidate.selected);
    assert!(!candidate.accepted);
    assert_eq!(
        candidate.reasons,
        vec![extractor_document_ir::QualityReason::TooShort]
    );
    Ok(())
}

#[test]
fn self_contained_low_quality_still_degrades() -> Result<(), Box<dyn std::error::Error>> {
    // Without an external URL a below-threshold story keeps the degradation contract.
    let bytes =
        br#"{"id": 902, "title": "Bare self-contained story", "text": null, "children": []}"#;
    let Err(ProviderError::LowQuality { candidates }) =
        from_provider(test_input(SourceRoute::HackerNews, bytes)?, limits())
    else {
        return Err("a self-contained low-quality story must degrade".into());
    };
    let candidate = candidates.first().ok_or("candidate evidence is attached")?;
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidate.strategy, "hacker_news_item");
    assert!(!candidate.selected);
    assert!(!candidate.accepted);
    assert_eq!(
        candidate.reasons,
        vec![extractor_document_ir::QualityReason::TooShort]
    );
    Ok(())
}

#[test]
fn oversized_payload_is_resource_limit() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/oversized-padded.json");
    let tight = ProviderLimits {
        max_input_bytes: 16_384,
        max_blocks: 2_000,
    };
    let outcome = from_provider(test_input(SourceRoute::HackerNews, bytes)?, tight);
    assert!(matches!(outcome, Err(ProviderError::ResourceLimit)));

    let generous_blocks = ProviderLimits {
        max_input_bytes: 8 * 1_024 * 1_024,
        max_blocks: 10,
    };
    let outcome = from_provider(test_input(SourceRoute::HackerNews, bytes)?, generous_blocks);
    assert!(matches!(outcome, Err(ProviderError::ResourceLimit)));
    Ok(())
}

fn limits() -> ProviderLimits {
    ProviderLimits {
        max_input_bytes: 65_536,
        max_blocks: 2_000,
    }
}

fn test_input(
    route: SourceRoute,
    bytes: &'static [u8],
) -> Result<ProviderInput<'static>, Box<dyn std::error::Error>> {
    Ok(ProviderInput {
        document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000043")?,
        source_address: DocumentAddress::parse("https://example.test/native")?,
        source_blob: blob_ref(bytes.len())?,
        route,
        bytes,
    })
}

fn blob_ref(length: usize) -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"be".repeat(32))?,
        },
        media_type: MediaType::parse("application/json")?,
        length_bytes: u64::try_from(length)?,
    })
}
