//! Parse-once candidate selection behavior.

use extractor_document_ir::{HtmlDocumentInput, ParseLimits, from_html};
use ratatoskr_document_contracts::{DocumentAddress, DocumentBlock};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn semantic_article_beats_page_chrome() -> Result<(), Box<dyn std::error::Error>> {
    let extraction = from_html(
        HtmlDocumentInput {
            document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000005")?,
            source_address: DocumentAddress::parse("https://example.com/noisy")?,
            source_blob: blob_ref()?,
            bytes: br"<!doctype html><html><head><title>Forest Dispatch</title></head><body>
                <nav><p>Nav products pricing company account</p></nav>
                <article><h1>Forest Dispatch</h1><p>The first field report contains enough useful article text to describe the route through the forest in detail.</p><p>The second report confirms the destination and records the evidence needed by every later reader.</p></article>
                <footer><p>Footer privacy terms careers cookies sitemap</p></footer>
            </body></html>",
        },
        ParseLimits {
            max_input_bytes: 4_096,
            max_dom_nodes: 128,
        },
    )?;

    assert_eq!(
        block_texts(&extraction.document.blocks),
        [
            "Forest Dispatch",
            "The first field report contains enough useful article text to describe the route through the forest in detail.",
            "The second report confirms the destination and records the evidence needed by every later reader.",
        ]
    );
    assert_eq!(
        extraction
            .candidates
            .iter()
            .map(|candidate| candidate.strategy.as_str())
            .collect::<Vec<_>>(),
        ["semantic", "readability", "density"]
    );
    assert!(
        extraction
            .document
            .provenance
            .iter()
            .all(|evidence| evidence.extraction_strategy.as_str() == "semantic")
    );
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

fn blob_ref() -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            )?,
        },
        media_type: MediaType::parse("text/html")?,
        length_bytes: 512,
    })
}
