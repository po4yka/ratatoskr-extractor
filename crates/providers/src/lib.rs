#![forbid(unsafe_code)]

//! Bounded provider-native JSON conversion into the shared Document IR contract.
use ratatoskr_document_contracts::{Document, DocumentAddress, ExtractionStrategy};

use extractor_document_ir::{CandidateDecision, assemble_document};
use ratatoskr_identifiers::{BlobRef, DocumentId};

mod hacker_news;
mod reddit;

pub use hacker_news::HACKER_NEWS_STRATEGY;
pub use reddit::REDDIT_STRATEGY;

/// The provider routes this crate knows how to represent natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRoute {
    /// Hacker News items served by the Algolia public API.
    HackerNews,
    /// Reddit permalinks served as JSON listings.
    Reddit,
}

/// Inputs needed to construct a shared document from a verified provider payload.
#[derive(Debug, Clone)]
pub struct ProviderInput<'a> {
    /// Stable document identity assigned by the extraction run.
    pub document_id: DocumentId,
    /// Address of the fetched native representation.
    pub source_address: DocumentAddress,
    /// Verified raw payload artifact.
    pub source_blob: BlobRef,
    /// Which provider schema the payload follows.
    pub route: SourceRoute,
    /// Raw payload bytes.
    pub bytes: &'a [u8],
}

/// Finite adapter budgets.
#[derive(Debug, Clone, Copy)]
pub struct ProviderLimits {
    /// Maximum payload bytes admitted to an adapter.
    pub max_input_bytes: usize,
    /// Maximum Document IR blocks produced by one conversion.
    pub max_blocks: usize,
}

/// Selected Document IR built from one provider payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderExtraction {
    /// Shared Document IR built from the extracted content.
    pub document: Document,
    /// Candidate decisions in stable strategy order.
    pub candidates: Vec<CandidateDecision>,
}

/// Converts one verified provider payload into shared Document IR.
///
/// # Errors
///
/// Returns [`ProviderError`] when a budget is exceeded or the payload violates its schema.
pub fn from_provider(
    input: ProviderInput<'_>,
    limits: ProviderLimits,
) -> Result<ProviderExtraction, ProviderError> {
    if input.bytes.len() > limits.max_input_bytes {
        return Err(ProviderError::ResourceLimit);
    }
    let strategy;
    let (title, _blocks, mut decision) = match input.route {
        SourceRoute::HackerNews => {
            strategy = HACKER_NEWS_STRATEGY;
            hacker_news::from_algolia(input.bytes, &limits)?
        }
        SourceRoute::Reddit => {
            strategy = REDDIT_STRATEGY;
            reddit::from_listings(input.bytes, &limits)?
        }
    };
    if !decision.accepted {
        return Err(ProviderError::LowQuality {
            candidates: vec![decision],
        });
    }
    decision.selected = true;
    let extraction_strategy =
        ExtractionStrategy::parse(strategy).map_err(|_| ProviderError::Schema)?;
    let document = assemble_document(
        input.document_id,
        input.source_address,
        &input.source_blob,
        &extraction_strategy,
        title,
        None,
        decision.blocks.clone(),
    )
    .map_err(provider_identity_error)?;
    Ok(ProviderExtraction {
        document,
        candidates: vec![decision],
    })
}

fn provider_identity_error(error: extractor_document_ir::DocumentIrError) -> ProviderError {
    match error {
        extractor_document_ir::DocumentIrError::Serialization(serialization) => {
            ProviderError::Serialization(serialization)
        }
        _ => ProviderError::InvalidUrl,
    }
}

/// Maps a classified source URL to its provider-native representation.
///
/// Hacker News item URLs map to their Algolia public-API endpoint; Reddit comment permalinks on
/// the canonical web hosts map to the same permalink with a `.json` suffix. Returns `None` when
/// the URL does not match a documented shape, in which case callers take the ordinary HTML path
/// with the original URL.
///
/// # Errors
///
/// Returns [`ProviderError`] when the URL cannot be parsed at all.
pub fn provider_request(
    route: SourceRoute,
    url: &str,
) -> Result<Option<DocumentAddress>, ProviderError> {
    let parsed = url::Url::parse(url).map_err(|_| ProviderError::InvalidUrl)?;
    match route {
        SourceRoute::HackerNews => hacker_news_request(&parsed),
        SourceRoute::Reddit => reddit_request(&parsed),
    }
}

fn hacker_news_request(parsed: &url::Url) -> Result<Option<DocumentAddress>, ProviderError> {
    if !host_is(parsed.host_str(), "news.ycombinator.com") || parsed.path() != "/item" {
        return Ok(None);
    }
    let id = parsed
        .query_pairs()
        .find(|(key, _)| key == "id")
        .map(|(_, value)| value);
    let Some(id) = id else {
        return Ok(None);
    };
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let endpoint = format!("https://hn.algolia.com/api/v1/items/{id}");
    DocumentAddress::parse(&endpoint)
        .map(Some)
        .map_err(|_| ProviderError::InvalidUrl)
}

fn reddit_request(parsed: &url::Url) -> Result<Option<DocumentAddress>, ProviderError> {
    let canonical_web_host = matches!(parsed.host_str(), Some("www.reddit.com" | "reddit.com"));
    if !canonical_web_host {
        return Ok(None);
    }
    let segments = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let [kind, subreddit, comments, tail @ ..] = segments.as_slice() else {
        return Ok(None);
    };
    if kind != &"r"
        || comments != &"comments"
        || subreddit.is_empty()
        || tail.iter().any(|segment| segment.is_empty())
    {
        return Ok(None);
    }
    let mut mapped = parsed.clone();
    mapped.set_fragment(None);
    let mut json_path = mapped.path().to_owned();
    if !json_path.ends_with('/') {
        json_path.push('/');
    }
    json_path.push_str(".json");
    mapped.set_path(&json_path);
    DocumentAddress::parse(mapped.as_str())
        .map(Some)
        .map_err(|_| ProviderError::InvalidUrl)
}

fn host_is(host: Option<&str>, base: &str) -> bool {
    host.is_some_and(|host| {
        host == base
            || host
                .strip_suffix(base)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

/// Why verified provider payloads could not become Document IR.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// Input bytes or produced blocks exceeded a finite adapter budget.
    #[error("provider adapter resource limit exceeded")]
    ResourceLimit,
    /// The payload is valid JSON but does not satisfy the required schema.
    #[error("provider payload does not satisfy the schema")]
    Schema,
    /// The URL itself is malformed.
    #[error("provider source URL is invalid")]
    InvalidUrl,
    /// The converted content did not cross the shared quality thresholds; rejected candidate
    /// evidence is attached for the degraded terminal record.
    #[error("provider content did not meet quality thresholds")]
    LowQuality {
        /// Rejected candidate decisions retained for diagnostics and persistence.
        candidates: Vec<CandidateDecision>,
    },
    /// Canonical block serialization failed.
    #[error("Document IR blocks could not be serialized")]
    Serialization(#[from] serde_json::Error),
}
