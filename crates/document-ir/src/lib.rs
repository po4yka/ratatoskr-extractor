#![forbid(unsafe_code)]

//! Bounded parse-once HTML conversion into the shared Document IR contract.

mod dom;

use ratatoskr_document_contracts::{
    Document, DocumentAddress, DocumentBlock, DocumentProvenance, ExtractionStrategy, LanguageTag,
};
use ratatoskr_identifiers::{BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId};
use sha2::{Digest as _, Sha256};

use crate::dom::{Element, HtmlDom};

/// Finite parser budgets.
#[derive(Debug, Clone, Copy)]
pub struct ParseLimits {
    /// Maximum source bytes accepted by this parser.
    pub max_input_bytes: usize,
    /// Maximum DOM nodes accepted after HTML5 recovery.
    pub max_dom_nodes: usize,
}

/// Inputs needed to construct a shared document.
#[derive(Debug, Clone)]
pub struct HtmlDocumentInput<'a> {
    /// Stable document identity assigned by the extraction run.
    pub document_id: DocumentId,
    /// Final source address.
    pub source_address: DocumentAddress,
    /// Verified raw source artifact.
    pub source_blob: BlobRef,
    /// HTML source bytes.
    pub bytes: &'a [u8],
}

/// Why HTML could not become Document IR.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DocumentIrError {
    /// Input or recovered DOM exceeded a finite parser budget.
    #[error("HTML parser resource limit exceeded")]
    ResourceLimit,
    /// A service-owned contract value could not be constructed.
    #[error("Document IR identity could not be constructed")]
    InvalidIdentity,
    /// Canonical block serialization failed.
    #[error("Document IR blocks could not be serialized")]
    Serialization(#[from] serde_json::Error),
}

/// Parses one HTML document and returns shared Document IR.
///
/// # Errors
///
/// Returns [`DocumentIrError`] when the shared contract cannot be constructed.
pub fn from_html(
    input: HtmlDocumentInput<'_>,
    limits: ParseLimits,
) -> Result<Document, DocumentIrError> {
    if input.bytes.len() > limits.max_input_bytes {
        return Err(DocumentIrError::ResourceLimit);
    }
    let source = String::from_utf8_lossy(input.bytes);
    let dom = HtmlDom::parse(&source);
    if dom.node_count() > limits.max_dom_nodes {
        return Err(DocumentIrError::ResourceLimit);
    }
    let mut title = None;
    let mut language = None;
    let mut blocks = Vec::new();

    for element in dom.elements() {
        let name = element.name();
        if name == "html" && language.is_none() {
            language = element
                .attr("lang")
                .and_then(|raw| LanguageTag::parse(raw).ok());
        }
        if name == "title" && title.is_none() {
            title = normalized_text(element);
        }
        let block = match name {
            "h1" => heading(1, element),
            "h2" => heading(2, element),
            "h3" => heading(3, element),
            "h4" => heading(4, element),
            "h5" => heading(5, element),
            "h6" => heading(6, element),
            "p" => normalized_text(element).map(|text| DocumentBlock::Paragraph { text }),
            _ => None,
        };
        if let Some(block) = block {
            blocks.push(block);
        }
    }

    let canonical = ratatoskr_identifiers::canonical_json(&blocks)?;
    let digest = hex(&Sha256::digest(canonical.as_bytes()));
    let strategy = ExtractionStrategy::parse("html_primitives")
        .map_err(|_| DocumentIrError::InvalidIdentity)?;
    let provenance = blocks
        .iter()
        .enumerate()
        .map(|(index, _)| {
            Ok(DocumentProvenance {
                block_index: u32::try_from(index).map_err(|_| DocumentIrError::InvalidIdentity)?,
                extraction_strategy: strategy.clone(),
                source_blob: input.source_blob.clone(),
            })
        })
        .collect::<Result<Vec<_>, DocumentIrError>>()?;
    Ok(Document {
        document_id: input.document_id,
        source_address: input.source_address,
        content_digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&digest).map_err(|_| DocumentIrError::InvalidIdentity)?,
        },
        title,
        language,
        blocks,
        provenance,
    })
}

fn heading(level: u8, element: Element<'_>) -> Option<DocumentBlock> {
    normalized_text(element).map(|text| DocumentBlock::Heading { level, text })
}

fn normalized_text(element: Element<'_>) -> Option<String> {
    let text = element
        .text()
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() { None } else { Some(text) }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}
