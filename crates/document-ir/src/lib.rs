#![forbid(unsafe_code)]

//! Bounded parse-once HTML conversion into the shared Document IR contract.

mod candidate;
mod dom;

use ratatoskr_document_contracts::{
    Document, DocumentAddress, DocumentBlock, DocumentProvenance, ExtractionStrategy, LanguageTag,
};
use ratatoskr_identifiers::{BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId};
use sha2::{Digest as _, Sha256};

use crate::dom::{Element, HtmlDom};

/// Fixed-point components recorded for one quality decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityMetrics {
    /// Normalized candidate characters.
    pub text_characters: u32,
    /// Paragraph blocks in the candidate.
    pub paragraph_count: u16,
    /// Text-volume contribution, at most 300.
    pub text_volume: u16,
    /// Paragraph-distribution contribution, at most 200.
    pub paragraph_distribution: u16,
    /// Non-link-text contribution, at most 200.
    pub non_link_share: u16,
    /// Non-boilerplate-text contribution, at most 200.
    pub non_boilerplate_share: u16,
    /// Title-agreement contribution, at most 100.
    pub title_agreement: u16,
}

/// Stable explanation attached to one quality score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityReason {
    /// Candidate meets the current acceptance thresholds.
    Accepted,
    /// Candidate does not contain the minimum normalized text.
    TooShort,
    /// Candidate score is below the current threshold.
    BelowThreshold,
}

/// One named in-memory candidate produced from the shared DOM.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateDecision {
    /// Stable extraction strategy name.
    pub strategy: String,
    /// Ordered blocks proposed by the strategy.
    pub blocks: Vec<DocumentBlock>,
    /// Stable evaluator implementation version.
    pub evaluator_version: &'static str,
    /// Bounded fixed-point quality components.
    pub metrics: QualityMetrics,
    /// Total score from 0 through 1000.
    pub score: u16,
    /// Whether this candidate meets both acceptance thresholds.
    pub accepted: bool,
    /// Stable bounded decision reasons.
    pub reasons: Vec<QualityReason>,
    /// Whether this candidate supplied the output Document IR.
    pub selected: bool,
}

/// Selected Document IR plus every candidate considered from the same DOM.
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlExtraction {
    /// Shared Document IR built from the selected candidate.
    pub document: Document,
    /// Candidate decisions in stable strategy priority order.
    pub candidates: Vec<CandidateDecision>,
}

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
    /// No candidate met the bounded quality thresholds.
    #[error("HTML candidate quality is below the acceptance threshold")]
    LowQuality {
        /// Rejected candidate decisions retained for diagnostics and persistence.
        candidates: Vec<CandidateDecision>,
    },
}

/// Parses one HTML document and returns shared Document IR.
///
/// # Errors
///
/// Returns [`DocumentIrError`] when the shared contract cannot be constructed.
pub fn from_html(
    input: HtmlDocumentInput<'_>,
    limits: ParseLimits,
) -> Result<HtmlExtraction, DocumentIrError> {
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
    }

    let candidates = candidate::extract(&dom);
    let mut decisions = candidates
        .iter()
        .map(|candidate| evaluate(candidate, title.as_deref()))
        .collect::<Vec<_>>();
    let selected_strategy = winner(&decisions, true)
        .and_then(|decision| {
            candidates
                .iter()
                .find(|candidate| candidate.strategy.as_str() == decision.strategy)
        })
        .map(|candidate| candidate.strategy);
    let Some(selected_strategy) = selected_strategy else {
        return Err(DocumentIrError::LowQuality {
            candidates: decisions,
        });
    };
    let selected = candidates
        .iter()
        .find(|candidate| candidate.strategy == selected_strategy)
        .ok_or(DocumentIrError::InvalidIdentity)?;
    for decision in &mut decisions {
        decision.selected = decision.strategy == selected_strategy.as_str();
    }
    let blocks = selected.blocks.clone();

    let canonical = ratatoskr_identifiers::canonical_json(&blocks)?;
    let digest = hex(&Sha256::digest(canonical.as_bytes()));
    let strategy = ExtractionStrategy::parse(selected.strategy.as_str())
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
    Ok(HtmlExtraction {
        document: Document {
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
        },
        candidates: decisions,
    })
}

fn evaluate(candidate: &candidate::Candidate, title: Option<&str>) -> CandidateDecision {
    let text_characters = candidate.blocks.iter().map(block_text_len).sum::<usize>();
    let paragraph_count = candidate
        .blocks
        .iter()
        .filter(|block| matches!(block, DocumentBlock::Paragraph { .. }))
        .count();
    let metrics = QualityMetrics {
        text_characters: u32::try_from(text_characters).unwrap_or(u32::MAX),
        paragraph_count: u16::try_from(paragraph_count).unwrap_or(u16::MAX),
        text_volume: component(text_characters, 300, 300),
        paragraph_distribution: component(paragraph_count, 4, 200),
        non_link_share: share(text_characters, candidate.link_characters, 200),
        non_boilerplate_share: share(text_characters, candidate.boilerplate_characters, 200),
        title_agreement: u16::from(title_matches(&candidate.blocks, title)) * 100,
    };
    let score = metrics
        .text_volume
        .saturating_add(metrics.paragraph_distribution)
        .saturating_add(metrics.non_link_share)
        .saturating_add(metrics.non_boilerplate_share)
        .saturating_add(metrics.title_agreement);
    let accepted = text_characters >= 120 && score >= 350;
    let reasons = if accepted {
        vec![QualityReason::Accepted]
    } else {
        let mut reasons = Vec::with_capacity(2);
        if text_characters < 120 {
            reasons.push(QualityReason::TooShort);
        }
        if score < 350 {
            reasons.push(QualityReason::BelowThreshold);
        }
        reasons
    };
    CandidateDecision {
        strategy: candidate.strategy.as_str().to_owned(),
        blocks: candidate.blocks.clone(),
        evaluator_version: "quality_v1",
        metrics,
        score,
        accepted,
        reasons,
        selected: false,
    }
}

fn winner(decisions: &[CandidateDecision], accepted_only: bool) -> Option<&CandidateDecision> {
    decisions
        .iter()
        .filter(|decision| !accepted_only || decision.accepted)
        .fold(None, |best: Option<&CandidateDecision>, decision| {
            if best.is_none_or(|best| decision.score > best.score) {
                Some(decision)
            } else {
                best
            }
        })
}

fn component(value: usize, full_value: usize, weight: u16) -> u16 {
    let weighted = value.min(full_value).saturating_mul(usize::from(weight)) / full_value;
    u16::try_from(weighted).unwrap_or(weight)
}

fn share(total: usize, excluded: usize, weight: u16) -> u16 {
    if total == 0 {
        return 0;
    }
    let weighted = total
        .saturating_sub(excluded.min(total))
        .saturating_mul(usize::from(weight))
        / total;
    u16::try_from(weighted).unwrap_or(weight)
}

fn title_matches(blocks: &[DocumentBlock], title: Option<&str>) -> bool {
    title.is_some_and(|title| {
        blocks.iter().any(|block| {
            matches!(block, DocumentBlock::Heading { text, .. } if text.eq_ignore_ascii_case(title))
        })
    })
}

fn block(element: Element<'_>) -> Option<DocumentBlock> {
    match element.name() {
        "h1" => heading(1, element),
        "h2" => heading(2, element),
        "h3" => heading(3, element),
        "h4" => heading(4, element),
        "h5" => heading(5, element),
        "h6" => heading(6, element),
        "p" => normalized_text(element).map(|text| DocumentBlock::Paragraph { text }),
        _ => None,
    }
}

fn heading(level: u8, element: Element<'_>) -> Option<DocumentBlock> {
    normalized_text(element).map(|text| DocumentBlock::Heading { level, text })
}

fn block_text_len(block: &DocumentBlock) -> usize {
    match block {
        DocumentBlock::Heading { text, .. } | DocumentBlock::Paragraph { text } => text.len(),
        _ => 0,
    }
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
