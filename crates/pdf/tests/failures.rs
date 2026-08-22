//! Typed failure modes: encryption, budgets, text-less pages, and hostile structure.

use extractor_pdf::{PdfDocumentInput, PdfError, PdfParseLimits, from_pdf};
use ratatoskr_document_contracts::{DocumentAddress, DocumentBlock};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn password_required_pdf_is_typed_encrypted() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/encrypted-user-password.pdf");
    let Err(error) = from_pdf(test_input(bytes)?, generous_limits()) else {
        return Err("a password-required PDF must not extract".into());
    };
    match error {
        PdfError::Encrypted => Ok(()),
        other => Err(format!("expected the typed encryption failure, got: {other}").into()),
    }
}

#[test]
fn blank_password_encrypted_pdf_extracts() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/encrypted-blank-password.pdf");
    let extraction = from_pdf(test_input(bytes)?, generous_limits())?;
    let Some(DocumentBlock::Paragraph { text }) = extraction.document.blocks.first().cloned()
    else {
        return Err("blank-password fixture must yield one paragraph block".into());
    };
    assert_eq!(
        text,
        "This document ships with an empty user password. Ordinary readers may therefore decrypt and extract it without any prompt. The direct extraction path must accept this content after decryption."
    );
    assert!(
        extraction
            .candidates
            .first()
            .is_some_and(|candidate| candidate.accepted)
    );
    Ok(())
}

#[test]
fn oversized_input_is_resource_limit() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/oversized-padded.pdf");
    let limits = PdfParseLimits {
        max_input_bytes: 16_384,
        max_pages: 10,
        max_text_bytes: 1_048_576,
    };
    let Err(error) = from_pdf(test_input(bytes)?, limits) else {
        return Err("oversized input must not extract".into());
    };
    match error {
        PdfError::ResourceLimit => Ok(()),
        other => Err(format!("expected the typed resource limit, got: {other}").into()),
    }
}

#[test]
fn page_and_text_budgets_stop_extraction() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/oversized-padded.pdf");
    let page_limits = PdfParseLimits {
        max_input_bytes: 65_536,
        max_pages: 0,
        max_text_bytes: 1_048_576,
    };
    let Err(error) = from_pdf(test_input(bytes)?, page_limits) else {
        return Err("the page budget must stop extraction".into());
    };
    assert!(matches!(error, PdfError::ResourceLimit));

    let text_limits = PdfParseLimits {
        max_input_bytes: 65_536,
        max_pages: 10,
        max_text_bytes: 1_024,
    };
    let Err(error) = from_pdf(test_input(bytes)?, text_limits) else {
        return Err("the extracted-text budget must stop extraction".into());
    };
    assert!(matches!(error, PdfError::ResourceLimit));
    Ok(())
}

#[test]
fn no_text_layer_pdf_degrades_with_candidates() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/no-text-layer.pdf");
    let Err(error) = from_pdf(test_input(bytes)?, generous_limits()) else {
        return Err("a text-less PDF must not produce Document IR".into());
    };
    let PdfError::NoTextLayer { candidates } = error else {
        return Err(format!("expected the typed degradation, got: {error}").into());
    };
    let Some(candidate) = candidates.first() else {
        return Err("rejected candidate evidence must be attached".into());
    };
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidate.strategy, "direct_pdf");
    assert!(!candidate.selected);
    assert!(!candidate.accepted);
    assert_eq!(candidate.metrics.text_characters, 0);
    assert_eq!(
        candidate.reasons,
        vec![
            extractor_document_ir::QualityReason::TooShort,
            extractor_document_ir::QualityReason::BelowThreshold,
        ]
    );
    Ok(())
}

#[test]
fn corrupt_pdf_is_malformed_not_panic() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = include_bytes!("fixtures/corrupt-truncated.pdf");
    let Err(error) = from_pdf(test_input(bytes)?, generous_limits()) else {
        return Err("a corrupt PDF must not extract".into());
    };
    match error {
        PdfError::Malformed => Ok(()),
        other => Err(format!("expected the typed malformed failure, got: {other}").into()),
    }
}

fn generous_limits() -> PdfParseLimits {
    PdfParseLimits {
        max_input_bytes: 65_536,
        max_pages: 10,
        max_text_bytes: 1_048_576,
    }
}

fn test_input(
    bytes: &'static [u8],
) -> Result<PdfDocumentInput<'static>, Box<dyn std::error::Error>> {
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
            hex: DigestHex::parse(&"ab".repeat(32))?,
        },
        media_type: MediaType::parse("application/pdf")?,
        length_bytes: u64::try_from(length)?,
    })
}
