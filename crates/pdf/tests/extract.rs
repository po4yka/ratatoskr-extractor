//! Direct PDF extraction into shared Document IR.

use extractor_pdf::{PdfDocumentInput, PdfParseLimits, from_pdf};
use ratatoskr_document_contracts::{DocumentAddress, DocumentBlock};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn text_pdf_yields_page_ordered_paragraphs() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/text-two-pages.pdf");
    let input = PdfDocumentInput {
        document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000007")?,
        source_address: DocumentAddress::parse("https://example.com/report.pdf")?,
        source_blob: blob_ref(bytes.len(), "application/pdf")?,
        bytes,
    };
    let limits = PdfParseLimits {
        max_input_bytes: 65_536,
        max_pages: 10,
        max_text_bytes: 1_048_576,
    };

    let extraction = from_pdf(input.clone(), limits)?;
    assert_eq!(
        block_texts(&extraction.document.blocks),
        [
            "Ratatoskr direct extraction fixture. The first page carries deterministic prose for the parser. Un texte de contrôle avec accents français stables.",
            "Second page follows the first in the page tree. Reading order must keep this page after page one.",
        ]
    );
    assert_eq!(extraction.document.document_id, input.document_id);
    assert_eq!(extraction.document.source_address, input.source_address);
    assert_eq!(
        extraction.document.title.as_deref(),
        Some("Direct Extraction Fixture")
    );
    assert!(extraction.document.language.is_none());

    assert_eq!(
        extraction.document.content_digest.algorithm,
        DigestAlgorithm::Sha256
    );
    assert_eq!(
        extraction.document.content_digest.hex.as_str(),
        from_pdf(input.clone(), limits)?
            .document
            .content_digest
            .hex
            .as_str()
    );

    let provenance = &extraction.document.provenance;
    assert_eq!(provenance.len(), extraction.document.blocks.len());
    for (index, entry) in provenance.iter().enumerate() {
        assert_eq!(entry.block_index, u32::try_from(index)?);
        assert_eq!(entry.extraction_strategy.as_str(), "direct_pdf");
        assert_eq!(entry.source_blob, input.source_blob);
    }

    assert_eq!(extraction.candidates.len(), 1);
    let candidate = extraction
        .candidates
        .first()
        .ok_or("expected exactly one candidate decision")?;
    assert_eq!(candidate.strategy, "direct_pdf");
    assert_eq!(candidate.evaluator_version, "quality_v1");
    assert!(candidate.accepted, "text fixture must cross the thresholds");
    assert!(candidate.selected);

    let again = from_pdf(input.clone(), limits)?;
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

fn blob_ref(length: usize, media_type: &str) -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"ab".repeat(32))?,
        },
        media_type: MediaType::parse(media_type)?,
        length_bytes: u64::try_from(length)?,
    })
}
