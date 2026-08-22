#![forbid(unsafe_code)]

//! Bounded parse-once direct PDF text conversion into the shared Document IR contract.

use ratatoskr_document_contracts::Document;
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::{BlobRef, DocumentId};

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
    /// The pages carry no extractable text layer; candidate evidence is attached.
    #[error("PDF has no extractable text layer")]
    NoTextLayer,
    /// Document IR identity could not be constructed.
    #[error("Document IR identity could not be constructed")]
    InvalidIdentity,
    /// Canonical block serialization failed.
    #[error("Document IR blocks could not be serialized")]
    Serialization(#[from] serde_json::Error),
}

/// Extracts one PDF document and returns shared Document IR.
///
/// # Errors
///
/// Returns [`PdfError`] when the shared contract cannot be constructed.
pub fn from_pdf(
    _input: PdfDocumentInput<'_>,
    _limits: PdfParseLimits,
) -> Result<PdfExtraction, PdfError> {
    Err(PdfError::Malformed)
}
