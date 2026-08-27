//! Deterministic output guarantees across repeated direct PDF extractions.

use extractor_pdf::{PdfDocumentInput, PdfParseLimits, from_pdf};
use ratatoskr_document_contracts::{DocumentAddress, DocumentBlock};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn multi_column_text_is_preserved_in_one_pass() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/multi-column.pdf");
    let limits = PdfParseLimits {
        max_input_bytes: 65_536,
        max_pages: 10,
        max_text_bytes: 1_048_576,
    };

    let extraction = from_pdf(test_input(bytes)?, limits)?;
    assert_eq!(
        block_texts(&extraction.document.blocks),
        [
            "Left column opens the article body here. Left column continues with more deterministic prose. Right column starts its own narrative thread. Right column closes the two-column fixture body."
        ]
    );
    assert_eq!(extraction.document.provenance.len(), 1);
    Ok(())
}

#[test]
fn repeated_extraction_is_byte_stable() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/text-two-pages.pdf");
    let limits = PdfParseLimits {
        max_input_bytes: 65_536,
        max_pages: 10,
        max_text_bytes: 1_048_576,
    };
    let input = test_input(bytes)?;

    let first = from_pdf(input.clone(), limits)?;
    let second = from_pdf(input.clone(), limits)?;
    assert_eq!(first, second);

    let third = from_pdf(input, limits)?;
    assert_eq!(third.document.content_digest, first.document.content_digest);
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

fn test_input(bytes: &[u8]) -> Result<PdfDocumentInput<'_>, Box<dyn std::error::Error>> {
    Ok(PdfDocumentInput {
        document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000007")?,
        source_address: DocumentAddress::parse("https://example.com/report.pdf")?,
        source_blob: blob_ref(bytes.len())?,
        bytes,
    })
}

fn blob_ref(length: usize) -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"cd".repeat(32))?,
        },
        media_type: MediaType::parse("application/pdf")?,
        length_bytes: u64::try_from(length)?,
    })
}
