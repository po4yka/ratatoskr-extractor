#![forbid(unsafe_code)]

//! Bounded `YouTube` transcript conversion into the shared Document IR contract.
//!
//! The crate is pure: it maps documented `YouTube` URL forms to one video identity, parses a
//! bounded embedded player response, selects a caption track under an explicit language
//! preference, parses the timed-text document, and builds Document IR together with an
//! extractor-owned timing sidecar. Network choreography lives in the service layer.

mod convert;
mod identity;
mod player;
pub mod sidecar;
mod timedtext;

use extractor_document_ir::CandidateDecision;
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::{BlobRef, DocumentId};

pub use convert::TranscriptExtraction;
pub use convert::from_transcript;
pub use identity::CanonicalWatchAddress;
pub use identity::IdentityError;
pub use identity::VideoIdentity;
pub use identity::resolve_identity;
pub use player::CaptionTrack;
pub use player::PlayerMeta;
pub use player::TrackKind;
pub use player::extract_player_response;
pub use player::select_track;
pub use timedtext::Segment;
pub use timedtext::parse_timedtext;

/// Stable extraction strategy recorded for the `YouTube` transcript path.
pub const YOUTUBE_STRATEGY: &str = "youtube_transcript";

/// Shared deterministic quality decision reused from the document-ir crate.
pub type CandidateDecisionAlias = CandidateDecision;

/// Finite budgets applied to every `YouTube` parsing stage.
#[derive(Debug, Clone, Copy)]
pub struct YoutubeLimits {
    /// Maximum watch-page bytes accepted for player-response extraction.
    pub max_page_bytes: usize,
    /// Maximum timed-text payload bytes accepted by the segment parsers.
    pub max_track_bytes: usize,
    /// Maximum number of blocks produced from one transcript.
    pub max_blocks: usize,
    /// Maximum number of segments accepted from one timed-text document.
    pub max_segments: usize,
    /// Maximum decoded text characters accepted for one segment.
    pub max_segment_characters: usize,
}

/// Inputs needed to construct a shared document from parsed transcript segments.
#[derive(Debug, Clone)]
pub struct TranscriptInput<'a> {
    /// Stable document identity assigned by the extraction run.
    pub document_id: DocumentId,
    /// Final source address of the canonical watch URL.
    pub source_address: DocumentAddress,
    /// Verified raw watch-page artifact.
    pub source_blob: BlobRef,
    /// Parsed video metadata used for title selection.
    pub meta: &'a PlayerMeta,
    /// Parsed transcript segments in cue order.
    pub segments: Vec<Segment>,
    /// Language tag of the selected caption track.
    pub language: &'a str,
}

/// Why `YouTube` content could not become Document IR through the transcript path.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum YoutubeError {
    /// A page, track payload, segment count, or block count exceeded a finite budget.
    #[error("YouTube parser resource limit exceeded")]
    ResourceLimit,
    /// The player response or timed-text payload violated its required schema.
    #[error("YouTube payload could not be parsed")]
    Schema,
    /// The video advertises no caption tracks at all.
    #[error("YouTube video has no caption tracks")]
    NoTranscript,
    /// Caption tracks exist but none matches the configured language preference.
    #[error("YouTube has no caption track matching the language preference")]
    NoLanguageMatch,
    /// Document IR identity could not be constructed.
    #[error("Document IR identity could not be constructed")]
    InvalidIdentity,
    /// Canonical block serialization failed.
    #[error("Document IR blocks could not be serialized")]
    Serialization(#[from] serde_json::Error),
}

/// One ordered transcript segment with millisecond timing.
///
/// This is the crate-internal conversion shape; see [`timedtext::Segment`].
pub type TranscriptSegment = Segment;
