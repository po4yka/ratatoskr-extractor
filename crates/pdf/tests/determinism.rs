//! Deterministic output guarantees across repeated direct PDF extractions.

use extractor_pdf::{PdfDocumentInput, PdfParseLimits, from_pdf};
use ratatoskr_document_contracts::{DocumentAddress, DocumentBlock};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};
use sha2::Digest as _;

#[test]
fn multi_column_text_is_preserved_in_one_pass() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/multi-column.pdf");
    let limits = PdfParseLimits {
        max_input_bytes: 65_536,
        max_pages: 10,
        max_text_bytes: 1_048_576,
    };

    let extraction = from_pdf(test_input(bytes)?, limits)?;
    let expected_blocks = vec![DocumentBlock::Paragraph {
        text: "Left column opens the article body here. Left column continues with more deterministic prose. Right column starts its own narrative thread. Right column closes the two-column fixture body."
            .to_owned(),
    }];
    assert_eq!(extraction.document.blocks, expected_blocks);
    assert_eq!(extraction.document.provenance.len(), 1);
    Ok(())
}

/// Blessing procedure: set every digit to `0`, run, then replace the expected hex with the
/// digest the failure prints. The constant pins the canonical hashing of the page blocks.
#[test]
fn repeated_extraction_is_byte_stable() -> Result<(), Box<dyn std::error::Error>> {
    const TEXT_TWO_PAGES_DIGEST: &str =
        "d3ae61d68b80d8734fa914068e16dfca76089e183ba08df8c1525ab837d68d37";

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
    let canonical = ratatoskr_identifiers::canonical_json(&third.document.blocks)?;
    let digest = format!("{:x}", sha2::Sha256::digest(canonical.as_bytes()));
    assert_eq!(digest, TEXT_TWO_PAGES_DIGEST);
    Ok(())
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
