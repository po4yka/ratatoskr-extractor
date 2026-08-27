//! Hacker News Algolia item schema and conversion.

use ratatoskr_document_contracts::DocumentBlock;

use extractor_document_ir::{
    evaluate_plain_text, heading_block, paragraph_block, plain_text_fragment,
};
use serde::Deserialize;

use crate::{AdapterExtraction, ProviderError, ProviderLimits};

/// Stable extraction strategy recorded for the Hacker News adapter.
pub const HACKER_NEWS_STRATEGY: &str = "hacker_news_item";

#[derive(Debug, Deserialize)]
pub(crate) struct AlgoliaItem {
    #[serde(default)]
    pub children: Vec<AlgoliaItem>,
    pub id: Option<i64>,
    pub text: Option<String>,
    pub title: Option<String>,
    /// Canonical external article URL for link posts.
    #[serde(default)]
    pub url: Option<String>,
}

/// Converts one Algolia item payload into its title, the canonical external article URL when the
/// item carries one, ordered blocks and the shared candidate decision.
///
/// # Errors
///
/// Returns [`ProviderError`] when the schema is violated or a budget is exceeded.
pub(crate) fn from_algolia(
    bytes: &[u8],
    limits: &ProviderLimits,
) -> Result<AdapterExtraction, ProviderError> {
    if bytes.len() > limits.max_input_bytes {
        return Err(ProviderError::ResourceLimit);
    }
    let root = serde_json::from_slice::<AlgoliaItem>(bytes).map_err(|_| ProviderError::Schema)?;
    if root.id.is_none() {
        return Err(ProviderError::Schema);
    }
    let title = required_title(&root)?;
    let mut blocks = vec![heading_block(1, title.clone())];
    push_fragment(root.text.as_deref(), limits, &mut blocks)?;
    collect_comments(&root.children, limits, &mut blocks)?;
    let decision = evaluate_plain_text(HACKER_NEWS_STRATEGY, &blocks, Some(&title));
    Ok((Some(title), root.url.clone(), blocks, decision))
}

fn required_title(root: &AlgoliaItem) -> Result<String, ProviderError> {
    let Some(title) = root.title.as_deref() else {
        return Err(ProviderError::Schema);
    };
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(ProviderError::Schema);
    }
    Ok(trimmed.to_owned())
}

fn collect_comments(
    children: &[AlgoliaItem],
    limits: &ProviderLimits,
    blocks: &mut Vec<DocumentBlock>,
) -> Result<(), ProviderError> {
    for child in children {
        push_fragment(child.text.as_deref(), limits, blocks)?;
        collect_comments(&child.children, limits, blocks)?;
    }
    Ok(())
}

fn push_fragment(
    html: Option<&str>,
    limits: &ProviderLimits,
    blocks: &mut Vec<DocumentBlock>,
) -> Result<(), ProviderError> {
    let Some(text) = html.and_then(plain_text_fragment) else {
        return Ok(());
    };
    if blocks.len() >= limits.max_blocks {
        return Err(ProviderError::ResourceLimit);
    }
    blocks.push(paragraph_block(text));
    Ok(())
}
