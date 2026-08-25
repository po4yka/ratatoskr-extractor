//! Source ownership classification tests.

use extractor_url_routing::{RoutingPolicy, SourceRoute, classify, normalize};

#[test]
fn known_hosts_win_without_matching_lookalikes() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        ("https://github.com/po4yka/ratatoskr", SourceRoute::GitHub),
        (
            "https://api.github.com/repos/po4yka/ratatoskr",
            SourceRoute::GitHub,
        ),
        ("https://x.com/user/status/1", SourceRoute::X),
        ("https://www.instagram.com/p/1/", SourceRoute::Instagram),
        ("https://www.threads.net/@user/post/1", SourceRoute::Threads),
        ("https://old.reddit.com/r/rust/", SourceRoute::Reddit),
        (
            "https://news.ycombinator.com/item?id=1",
            SourceRoute::HackerNews,
        ),
        ("https://youtu.be/example", SourceRoute::YouTube),
        (
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            SourceRoute::YouTube,
        ),
        (
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            SourceRoute::YouTube,
        ),
        (
            "https://m.youtube.com/shorts/dQw4w9WgXcQ",
            SourceRoute::YouTube,
        ),
        (
            "https://youtube-nocookie.com.example.test/watch?v=dQw4w9WgXcQ",
            SourceRoute::GenericWeb,
        ),
        ("https://cdn.example.com/report.PDF", SourceRoute::Pdf),
        (
            "https://github.com.example.test/project",
            SourceRoute::GenericWeb,
        ),
        ("https://notreddit.com/r/rust", SourceRoute::GenericWeb),
        ("https://example.com/article", SourceRoute::GenericWeb),
    ];
    let policy = RoutingPolicy::default();

    for (input, expected) in cases {
        let normalized = normalize(input, &policy)?;
        assert_eq!(classify(&normalized), expected, "classified {input}");
    }
    Ok(())
}
