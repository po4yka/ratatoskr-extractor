//! Deterministic candidate quality behavior.

use extractor_document_ir::{
    HtmlDocumentInput, ParseLimits, QualityMetrics, QualityReason, from_html,
};
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn evaluation_is_repeatable_with_stable_ties() -> Result<(), Box<dyn std::error::Error>> {
    let html = format!(
        "<html><head><title>Forest Notes</title></head><body><article><h1>Forest Notes</h1><p>{}</p></article></body></html>",
        "a".repeat(120)
    );
    let evaluate = || {
        from_html(
            HtmlDocumentInput {
                document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000006")?,
                source_address: DocumentAddress::parse("https://example.com/tie")?,
                source_blob: blob_ref()?,
                bytes: html.as_bytes(),
            },
            ParseLimits {
                max_input_bytes: 4_096,
                max_dom_nodes: 128,
            },
        )
        .map_err(Box::<dyn std::error::Error>::from)
    };

    let first = evaluate()?;
    let second = evaluate()?;
    assert_eq!(first.candidates, second.candidates);
    assert_eq!(first.candidates.len(), 3);
    for candidate in &first.candidates {
        assert_eq!(candidate.evaluator_version, "quality_v1");
        assert_eq!(
            candidate.metrics,
            QualityMetrics {
                text_characters: 132,
                paragraph_count: 1,
                text_volume: 66,
                paragraph_distribution: 50,
                non_link_share: 200,
                non_boilerplate_share: 200,
                title_agreement: 100,
            }
        );
        assert_eq!(candidate.score, 616);
        assert!(candidate.accepted);
        assert_eq!(candidate.reasons, [QualityReason::Accepted]);
    }
    let selected = first
        .candidates
        .iter()
        .find(|candidate| candidate.selected)
        .ok_or("no candidate was selected")?;
    assert_eq!(selected.strategy, "semantic");
    assert_eq!(
        first
            .candidates
            .iter()
            .filter(|candidate| candidate.selected)
            .count(),
        1
    );
    Ok(())
}

fn blob_ref() -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(
                "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            )?,
        },
        media_type: MediaType::parse("text/html")?,
        length_bytes: 256,
    })
}
