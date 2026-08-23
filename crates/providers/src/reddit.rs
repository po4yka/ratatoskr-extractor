//! Reddit link-plus-comments listing schema and conversion.

use ratatoskr_document_contracts::DocumentBlock;

use extractor_document_ir::CandidateDecision;

use crate::{ProviderError, ProviderLimits};

/// Stable extraction strategy recorded for the Reddit adapter.
pub const REDDIT_STRATEGY: &str = "reddit_post";

/// Converts one Reddit listing payload into ordered blocks and the shared candidate decision.
///
/// # Errors
///
/// Returns [`ProviderError`] when the schema is violated or a budget is exceeded.
pub(crate) fn from_listings(
    _bytes: &[u8],
    _limits: &ProviderLimits,
) -> Result<(Option<String>, Vec<DocumentBlock>, CandidateDecision), ProviderError> {
    Err(ProviderError::Schema)
}
