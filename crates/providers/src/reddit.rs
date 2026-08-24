//! Reddit link-plus-comments listing schema and conversion.

use ratatoskr_document_contracts::DocumentBlock;

use extractor_document_ir::evaluate_plain_text;
use serde::Deserialize;

use crate::{AdapterExtraction, ProviderError, ProviderLimits};

/// Stable extraction strategy recorded for the Reddit adapter.
pub const REDDIT_STRATEGY: &str = "reddit_post";

#[derive(Debug, Deserialize)]
struct Listing {
    data: ListingData,
}

#[derive(Debug, Deserialize)]
struct ListingData {
    #[serde(default)]
    children: Vec<Posting>,
}

#[derive(Debug, Deserialize)]
struct Posting {
    kind: String,
    data: PostingData,
}

#[derive(Debug, Deserialize)]
struct PostingData {
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    selftext: Option<String>,
    #[serde(default)]
    title: Option<String>,
    /// Canonical external article URL carried by link posts.
    #[serde(default)]
    url: Option<String>,
    /// Nested replies arrive either as an empty string or as a full listing; they stay
    /// untyped here and recurse through [`walk_replies`].
    #[serde(default)]
    replies: Option<serde_json::Value>,
}

/// Converts one Reddit listing payload into its title, the canonical external article URL for
/// link posts, ordered blocks and the shared candidate decision.
///
/// # Errors
///
/// Returns [`ProviderError`] when the schema is violated or a budget is exceeded.
pub(crate) fn from_listings(
    bytes: &[u8],
    limits: &ProviderLimits,
) -> Result<AdapterExtraction, ProviderError> {
    let listings =
        serde_json::from_slice::<Vec<Listing>>(bytes).map_err(|_| ProviderError::Schema)?;
    let mut postings = listings
        .into_iter()
        .flat_map(|listing| listing.data.children);
    let post = postings
        .find(|posting| posting.kind == "t3")
        .ok_or(ProviderError::Schema)?;
    let title = post
        .data
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or(ProviderError::Schema)?
        .to_owned();
    let external_url = post.data.url.clone();

    let mut blocks = vec![DocumentBlock::Heading {
        level: 1,
        text: title.clone(),
    }];
    push_text(post.data.selftext.as_deref(), limits, &mut blocks)?;

    let mut queue = std::collections::VecDeque::new();
    queue.push_back(post);
    while let Some(posting) = queue.pop_front() {
        if posting.kind == "t1" {
            push_text(posting.data.body.as_deref(), limits, &mut blocks)?;
        }
        if let Some(replies) = posting.data.replies {
            queue.extend(walk_replies(&replies));
        }
    }
    for posting in postings {
        if posting.kind == "t1" {
            push_text(posting.data.body.as_deref(), limits, &mut blocks)?;
        }
    }

    let decision = evaluate_plain_text(REDDIT_STRATEGY, &blocks, Some(&title));
    Ok((Some(title), external_url, blocks, decision))
}

/// Collects comment postings out of a replies value that is a full listing object.
fn walk_replies(replies: &serde_json::Value) -> Vec<Posting> {
    serde_json::from_value::<Listing>(replies.clone())
        .map(|listing| listing.data.children)
        .unwrap_or_default()
}

fn push_text(
    text: Option<&str>,
    limits: &ProviderLimits,
    blocks: &mut Vec<DocumentBlock>,
) -> Result<(), ProviderError> {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(());
    };
    if blocks.len() >= limits.max_blocks {
        return Err(ProviderError::ResourceLimit);
    }
    blocks.push(DocumentBlock::Paragraph {
        text: text.split_whitespace().collect::<Vec<_>>().join(" "),
    });
    Ok(())
}
