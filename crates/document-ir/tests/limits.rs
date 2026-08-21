//! Parser resource boundaries.

use extractor_document_ir::{DocumentIrError, HtmlDocumentInput, ParseLimits, from_html};
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn node_budget_refuses_pathological_html() -> Result<(), Box<dyn std::error::Error>> {
    let source = BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            )?,
        },
        media_type: MediaType::parse("text/html")?,
        length_bytes: 221,
    };
    let result = from_html(
        HtmlDocumentInput {
            document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000003")?,
            source_address: DocumentAddress::parse("https://example.com/pathological")?,
            source_blob: source,
            bytes: b"<html><body><div><span>a</span><span>b</span><span>c</span><span>d</span><span>e</span></div></body></html>",
        },
        ParseLimits {
            max_input_bytes: 4_096,
            max_dom_nodes: 5,
        },
    );

    assert!(matches!(result, Err(DocumentIrError::ResourceLimit)));
    Ok(())
}
