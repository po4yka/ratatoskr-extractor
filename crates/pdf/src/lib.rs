#![forbid(unsafe_code)]

//! Bounded parse-once direct PDF text conversion into the shared Document IR contract.
//!
//! Verified PDF bytes are parsed once, decrypted only when a blank user password suffices, and
//! walked page by page in page-tree order. Every failure mode is typed: budget overruns, required
//! passwords, malformed structure, and parser-internal panics never escape as crashes.

use std::panic::AssertUnwindSafe;

use extractor_document_ir::{CandidateDecision, evaluate_plain_text, paragraph_block};
use lopdf::{Document as PdfDocument, Object};
use ratatoskr_document_contracts::{Document, DocumentAddress, ExtractionStrategy};
use ratatoskr_identifiers::{BlobRef, DocumentId};

/// Stable extraction strategy recorded for the direct PDF path.
pub const DIRECT_PDF_STRATEGY: &str = "direct_pdf";

/// Inputs needed to construct a shared document from verified PDF bytes.
#[derive(Debug, Clone)]
pub struct PdfDocumentInput<'a> {
    /// Stable document identity assigned by the extraction run.
    pub document_id: DocumentId,
    /// Final source address.
    pub source_address: DocumentAddress,
    /// Verified raw source artifact.
    pub source_blob: BlobRef,
    /// PDF source bytes.
    pub bytes: &'a [u8],
}

/// Finite parser budgets.
#[derive(Debug, Clone, Copy)]
pub struct PdfParseLimits {
    /// Maximum source bytes accepted by this parser.
    pub max_input_bytes: usize,
    /// Maximum page count accepted from the page tree.
    pub max_pages: usize,
    /// Maximum accumulated extracted text bytes.
    pub max_text_bytes: usize,
}

/// Selected Document IR built from one direct PDF extraction.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfExtraction {
    /// Shared Document IR built from the extracted page text.
    pub document: Document,
    /// Candidate decisions in stable strategy order.
    pub candidates: Vec<CandidateDecision>,
}

/// Why verified PDF bytes could not become Document IR.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PdfError {
    /// Input, page count, or extracted text exceeded a finite parser budget.
    #[error("PDF parser resource limit exceeded")]
    ResourceLimit,
    /// The document requires a password that was not supplied.
    #[error("PDF requires a password")]
    Encrypted,
    /// The document is structurally invalid or crashed the underlying parser.
    #[error("PDF could not be parsed")]
    Malformed,
    /// The pages carry no extractable text layer; rejected candidate evidence is attached.
    #[error("PDF has no extractable text layer")]
    NoTextLayer {
        /// Rejected candidate decisions retained for diagnostics and persistence.
        candidates: Vec<CandidateDecision>,
    },
    /// Document IR identity could not be constructed.
    #[error("Document IR identity could not be constructed")]
    InvalidIdentity,
    /// Canonical block serialization failed.
    #[error("Document IR blocks could not be serialized")]
    Serialization(#[from] serde_json::Error),
}

/// Extracts one PDF document and returns shared Document IR.
///
/// The extraction is deterministic: identical bytes produce identical blocks, provenance, and
/// content digest. Page order is preserved as block order for every page that contributes text.
///
/// # Errors
///
/// Returns [`PdfError`] when a budget is exceeded, the document is encrypted or malformed, or the
/// shared contract cannot be constructed.
pub fn from_pdf(
    input: PdfDocumentInput<'_>,
    limits: PdfParseLimits,
) -> Result<PdfExtraction, PdfError> {
    if input.bytes.len() > limits.max_input_bytes {
        return Err(PdfError::ResourceLimit);
    }
    // The underlying parser panics on numerous hostile inputs. The boundary converts such panics
    // into the typed malformed failure; the closure owns every value it touches.
    let extracted = std::panic::catch_unwind(AssertUnwindSafe(|| parse_document(input, &limits)));
    extracted.unwrap_or(Err(PdfError::Malformed))
}

fn parse_document(
    input: PdfDocumentInput<'_>,
    limits: &PdfParseLimits,
) -> Result<PdfExtraction, PdfError> {
    let mut document = PdfDocument::load_mem(input.bytes).map_err(|_| PdfError::Malformed)?;
    if document.is_encrypted() {
        document
            .decrypt("")
            .map_err(|error| decryption_failure(&error))?;
    }
    let pages = document.get_pages();
    let page_count = pages.len();
    if page_count > limits.max_pages {
        return Err(PdfError::ResourceLimit);
    }
    let title = metadata_title(&document);

    let mut blocks = Vec::new();
    let mut text_bytes = 0usize;
    for page_number in 1..=u32::try_from(page_count).map_err(|_| PdfError::ResourceLimit)? {
        let page_text = extract_page_text(&document, page_number)?;
        text_bytes = text_bytes.saturating_add(page_text.len());
        if text_bytes > limits.max_text_bytes {
            return Err(PdfError::ResourceLimit);
        }
        if let Some(text) = normalized_paragraph(&page_text) {
            blocks.push(paragraph_block(text));
        }
    }

    let decision = evaluate_plain_text(DIRECT_PDF_STRATEGY, &blocks, title.as_deref());
    if !decision.accepted {
        return Err(PdfError::NoTextLayer {
            candidates: vec![decision],
        });
    }
    let mut selected = decision;
    selected.selected = true;
    let strategy =
        ExtractionStrategy::parse(DIRECT_PDF_STRATEGY).map_err(|_| PdfError::InvalidIdentity)?;
    let assembled = extractor_document_ir::assemble_document(
        input.document_id,
        input.source_address,
        &input.source_blob,
        &strategy,
        title,
        None,
        blocks,
    )
    .map_err(pdf_identity_error)?;
    Ok(PdfExtraction {
        document: assembled,
        candidates: vec![selected],
    })
}

fn extract_page_text(document: &PdfDocument, page_number: u32) -> Result<String, PdfError> {
    let mut text = String::new();
    {
        let mut output = pdf_extract::PlainTextOutput::new(&mut text);
        pdf_extract::output_doc_page(document, &mut output, page_number)
            .map_err(|_| PdfError::Malformed)?;
    }
    Ok(text)
}

fn decryption_failure(error: &lopdf::Error) -> PdfError {
    match error {
        lopdf::Error::Decryption(lopdf::encryption::DecryptionError::IncorrectPassword) => {
            PdfError::Encrypted
        }
        _ => PdfError::Malformed,
    }
}

fn pdf_identity_error(error: extractor_document_ir::DocumentIrError) -> PdfError {
    match error {
        extractor_document_ir::DocumentIrError::Serialization(serialization) => {
            PdfError::Serialization(serialization)
        }
        _ => PdfError::InvalidIdentity,
    }
}

fn metadata_title(document: &PdfDocument) -> Option<String> {
    let info = document.trailer.get(b"Info").ok()?;
    let info = resolved(document, info)?;
    let info = info.as_dict().ok()?;
    let title = info.get(b"Title").ok()?;
    let title = resolved(document, title)?;
    if !matches!(title, Object::String(_, _)) {
        return None;
    }
    let decoded = pdf_extract::decode_text_string(title).ok()?;
    let trimmed = decoded.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn resolved<'a>(document: &'a PdfDocument, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => document.get_object(*id).ok(),
        _ => Some(object),
    }
}

fn normalized_paragraph(page_text: &str) -> Option<String> {
    let text = page_text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { None } else { Some(text) }
}
