//! Transcript-to-Document conversion with deterministic block grouping.

use extractor_document_ir::{CandidateDecision, evaluate_plain_text};
use ratatoskr_document_contracts::{Document, DocumentBlock, ExtractionStrategy, LanguageTag};

use crate::Segment;
use crate::TranscriptInput;
use crate::YOUTUBE_STRATEGY;
use crate::YoutubeError;
use crate::YoutubeLimits;
use crate::sidecar::{BlockTiming, TimingSidecar};

/// Selected Document IR built from one transcript extraction.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptExtraction {
    /// Shared Document IR built from the transcript blocks.
    pub document: Document,
    /// Candidate decision in stable strategy order.
    pub candidate: crate::CandidateDecisionAlias,
    /// Extractor-owned timing diagnostics for every produced block.
    pub sidecar: TimingSidecar,
}

/// Converts parsed transcript segments into shared Document IR plus the timing sidecar.
///
/// Blocks map one-to-one to segments while the block budget lasts; past it, consecutive
/// segments merge deterministically and the sidecar records merged ranges. Group `g` of
/// `G` covers the half-open segment range `[g*n/G, (g+1)*n/G)` for `n` segments, and merged
/// texts join with a single space. The conversion is deterministic: identical inputs produce
/// identical documents, digests, and sidecars. Transcript text is authoritative structured
/// content, so the candidate decision is selected unconditionally; evaluator metrics are
/// recorded for observability but do not gate acceptance.
///
/// # Errors
///
/// Returns [`YoutubeError`] when the block budget is zero (`ResourceLimit`), the transcript
/// carries no segments (`Schema`), the language tag or another service-owned identity value
/// cannot be constructed (`InvalidIdentity`), or canonical block serialization fails
/// (`Serialization`).
pub fn from_transcript(
    input: TranscriptInput<'_>,
    limits: YoutubeLimits,
) -> Result<TranscriptExtraction, YoutubeError> {
    if limits.max_blocks == 0 {
        return Err(YoutubeError::ResourceLimit);
    }
    if input.segments.is_empty() {
        return Err(YoutubeError::Schema);
    }
    let title = normalized_title(input.meta.title.as_deref());
    let (blocks, timings) = grouped_blocks(&input.segments, limits.max_blocks)?;

    let decision = evaluate_plain_text(YOUTUBE_STRATEGY, &blocks, title.as_deref());
    let candidate = selected_transcript_decision(decision);

    let strategy =
        ExtractionStrategy::parse(YOUTUBE_STRATEGY).map_err(|_| YoutubeError::InvalidIdentity)?;
    let language = LanguageTag::parse(input.language).map_err(|_| YoutubeError::InvalidIdentity)?;
    let document = extractor_document_ir::assemble_document(
        input.document_id,
        input.source_address,
        &input.source_blob,
        &strategy,
        title,
        Some(language),
        blocks,
    )
    .map_err(identity_error)?;

    Ok(TranscriptExtraction {
        document,
        candidate,
        sidecar: TimingSidecar {
            strategy: YOUTUBE_STRATEGY.to_owned(),
            language: input.language.to_owned(),
            title: input.meta.title.clone(),
            channel: input.meta.channel.clone(),
            duration_seconds: input.meta.duration_seconds,
            segment_count: input.segments.len(),
            blocks: timings,
        },
    })
}

/// Groups ordered segments into at most `max_blocks` contiguous deterministic groups.
///
/// While every segment fits its own block, blocks map one-to-one to segments in cue order;
/// otherwise group `g` of `G` covers segments `[g*n/G, (g+1)*n/G)` and merges their texts
/// with single spaces. Every group is non-empty because `G <= n`, and each timing record
/// spans from its first segment's start to its last segment's end.
fn grouped_blocks(
    segments: &[Segment],
    max_blocks: usize,
) -> Result<(Vec<DocumentBlock>, Vec<BlockTiming>), YoutubeError> {
    let segment_count = segments.len();
    let group_count = max_blocks.min(segment_count);
    let mut blocks = Vec::with_capacity(group_count);
    let mut timings = Vec::with_capacity(group_count);
    for group in 0..group_count {
        let start_index = segment_count * group / group_count;
        let end_index = segment_count * (group + 1) / group_count;
        let mut text = String::new();
        let mut start_ms = 0_u64;
        let mut end_ms = 0_u64;
        for (position, segment) in segments
            .iter()
            .skip(start_index)
            .take(end_index - start_index)
            .enumerate()
        {
            if position > 0 {
                text.push(' ');
            }
            text.push_str(&segment.text);
            if position == 0 {
                start_ms = segment.start_ms;
            }
            end_ms = segment.start_ms.saturating_add(segment.duration_ms);
        }
        blocks.push(DocumentBlock::Paragraph { text });
        timings.push(BlockTiming {
            block_index: u32::try_from(group).map_err(|_| YoutubeError::ResourceLimit)?,
            start_ms,
            end_ms,
        });
    }
    Ok((blocks, timings))
}

/// Marks the transcript decision as selected unconditionally.
///
/// Transcript text is authoritative structured content supplied by the video itself: the
/// shared evaluator components are recorded for observability and cross-strategy comparison,
/// but they do not gate acceptance the way competing HTML candidates gate each other.
fn selected_transcript_decision(mut decision: CandidateDecision) -> CandidateDecision {
    decision.selected = true;
    decision
}

fn normalized_title(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Maps shared Document IR construction failures onto the transcript error surface.
///
/// Canonical block serialization keeps its dedicated variant; every other contract
/// construction failure is an identity failure, mirroring the direct PDF path.
fn identity_error(error: extractor_document_ir::DocumentIrError) -> YoutubeError {
    match error {
        extractor_document_ir::DocumentIrError::Serialization(serialization) => {
            YoutubeError::Serialization(serialization)
        }
        _ => YoutubeError::InvalidIdentity,
    }
}
