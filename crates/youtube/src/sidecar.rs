//! Extractor-owned timing sidecar for one transcript extraction.
//!
//! The shared Document contract carries plain paragraphs only. This module defines the
//! service-private record that maps every produced block index to the segment time range it
//! covers, so timing evidence stays recoverable inside the extractor without entering the
//! cross-repository shape.

use serde::Serialize;

/// Timing coverage of one produced block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockTiming {
    /// Zero-based index of the produced block.
    pub block_index: u32,
    /// First covered segment start in milliseconds.
    pub start_ms: u64,
    /// Last covered segment end in milliseconds.
    pub end_ms: u64,
}

/// Complete extractor-owned diagnostics for one successful transcript conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimingSidecar {
    /// Extraction strategy that produced the blocks.
    pub strategy: String,
    /// Selected caption track language tag.
    pub language: String,
    /// Video title when the player response supplied one.
    pub title: Option<String>,
    /// Channel name when the player response supplied one.
    pub channel: Option<String>,
    /// Video duration in seconds when the player response supplied one.
    pub duration_seconds: Option<u64>,
    /// Number of parsed source segments before block grouping.
    pub segment_count: usize,
    /// Per-block timing coverage in produced-block order.
    pub blocks: Vec<BlockTiming>,
}
