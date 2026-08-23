//! Mapping from classified source URLs to provider-native representations.

use extractor_providers::{SourceRoute, provider_request};

#[test]
fn provider_urls_map_to_native_representations() -> Result<(), Box<dyn std::error::Error>> {
    let hn = provider_request(
        SourceRoute::HackerNews,
        "https://news.ycombinator.com/item?id=900",
    )?
    .ok_or("an item URL must map")?;
    assert_eq!(
        hn.as_str(),
        "https://hn.algolia.com/api/v1/items/900",
        "the mapped endpoint is the sanctioned public API"
    );

    let reddit = provider_request(
        SourceRoute::Reddit,
        "https://www.reddit.com/r/fixturerust/comments/abc123/deterministic_extraction_fixture_post/",
    )?
    .ok_or("a comment permalink must map")?;
    assert_eq!(
        reddit.as_str(),
        "https://www.reddit.com/r/fixturerust/comments/abc123/deterministic_extraction_fixture_post/.json",
        "the native representation is the same permalink with a .json suffix"
    );
    Ok(())
}

#[test]
fn unmappable_shapes_return_none() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            SourceRoute::HackerNews,
            "https://news.ycombinator.com/newest",
        ),
        (
            SourceRoute::HackerNews,
            "https://news.ycombinator.com/item?id=not-numeric",
        ),
        (SourceRoute::Reddit, "https://www.reddit.com/r/fixturerust/"),
        (
            SourceRoute::Reddit,
            "https://old.reddit.com/r/fixturerust/comments/abc123/x/",
        ),
    ];
    for (route, url) in cases {
        let mapped = provider_request(route, url)?;
        assert!(mapped.is_none(), "{url} must not map to a representation");
    }
    Ok(())
}
