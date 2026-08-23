#![forbid(unsafe_code)]

//! Bounded provider-native JSON conversion into the shared Document IR contract.

use ratatoskr_document_contracts::DocumentAddress;

/// The provider routes this crate knows how to represent natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRoute {
    /// Hacker News items served by the Algolia public API.
    HackerNews,
    /// Reddit permalinks served as JSON listings.
    Reddit,
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
    let canonical_web_host = matches!(
        parsed.host_str(),
        Some("www.reddit.com") | Some("reddit.com")
    );
    if !canonical_web_host {
        return Ok(None);
    }
    let segments = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if segments.len() < 4 || segments[0] != "r" || segments[2] != "comments" {
        return Ok(None);
    }
    if segments[3..].iter().any(|segment| segment.is_empty()) {
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
}
