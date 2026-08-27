//! Reddit link-plus-comments listing conversion into shared Document IR.

use extractor_providers::{ProviderInput, ProviderLimits, SourceRoute, from_provider};
use ratatoskr_document_contracts::{DocumentAddress, DocumentBlock};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn reddit_post_and_comments_become_blocks() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/reddit-post.json");
    let input = ProviderInput {
        document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000042")?,
        source_address: DocumentAddress::parse(
            "https://www.reddit.com/r/fixturerust/comments/abc123/deterministic_extraction_fixture_post/.json",
        )?,
        source_blob: blob_ref(bytes.len())?,
        route: SourceRoute::Reddit,
        bytes,
    };
    let limits = ProviderLimits {
        max_input_bytes: 65_536,
        max_blocks: 2_000,
    };

    let extraction = from_provider(input.clone(), limits)?;
    assert_eq!(
        block_texts(&extraction.document.blocks),
        [
            "Deterministic extraction fixture post",
            "Body text of the synthetic fixture post survives conversion.",
            "Top-level comment body.",
            "Nested reply body.",
        ]
    );
    assert_eq!(extraction.document.document_id, input.document_id);
    assert_eq!(extraction.document.source_address, input.source_address);
    assert_eq!(
        extraction.document.title.as_deref(),
        Some("Deterministic extraction fixture post")
    );

    assert_eq!(
        extraction.document.provenance.len(),
        extraction.document.blocks.len()
    );
    for (index, entry) in extraction.document.provenance.iter().enumerate() {
        assert_eq!(entry.block_index, u32::try_from(index)?);
        assert_eq!(entry.extraction_strategy.as_str(), "reddit_post");
        assert_eq!(entry.source_blob, input.source_blob);
    }

    assert_eq!(extraction.candidates.len(), 1);
    let candidate = extraction
        .candidates
        .first()
        .ok_or("expected exactly one candidate decision")?;
    assert_eq!(candidate.strategy, "reddit_post");
    assert_eq!(candidate.evaluator_version, "quality_v1");
    assert!(candidate.accepted);
    assert!(candidate.selected);

    let again = from_provider(input.clone(), limits)?;
    assert_eq!(again, extraction);
    Ok(())
}

fn block_texts(blocks: &[DocumentBlock]) -> Vec<&str> {
    blocks
        .iter()
        .map(|block| match block {
            DocumentBlock::Heading { text, .. } | DocumentBlock::Paragraph { text, .. } => {
                text.as_str()
            }
            _ => "",
        })
        .collect()
}

fn blob_ref(length: usize) -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"cd".repeat(32))?,
        },
        media_type: MediaType::parse("application/json")?,
        length_bytes: u64::try_from(length)?,
    })
}
