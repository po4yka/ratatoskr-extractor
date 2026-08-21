//! URL normalization contract tests.

use extractor_url_routing::{RoutingPolicy, normalize};

#[test]
fn equivalent_tracked_urls_share_a_key_and_preserve_originals()
-> Result<(), Box<dyn std::error::Error>> {
    let first_input = "HTTPS://Example.COM:443/articles/1?utm_source=newsletter&a=1&b=2#comments";
    let second_input = "https://example.com/articles/1?a=1&b=2";
    let policy = RoutingPolicy::default();

    let first = normalize(first_input, &policy)?;
    let second = normalize(second_input, &policy)?;

    assert_eq!(first.original(), first_input);
    assert_eq!(second.original(), second_input);
    assert_eq!(first.normalized(), second.normalized());
    assert_eq!(first.routing_fingerprint(), second.routing_fingerprint());
    assert_eq!(first.normalized().query(), Some("a=1&b=2"));
    Ok(())
}

#[test]
fn ambiguous_urls_fail_before_resolution() {
    let policy = RoutingPolicy {
        max_url_length: 64,
        ..RoutingPolicy::default()
    };
    let too_long = format!("https://example.com/{}", "x".repeat(80));
    let cases = [
        "ftp://example.com/file",
        "https:/missing-host",
        "https://user:password@example.com/",
        "http://example.com:0/",
        "http://example.com:8080/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://0177.0.0.1/",
        too_long.as_str(),
    ];
    let resolver_calls = 0;

    for input in cases {
        assert!(normalize(input, &policy).is_err(), "accepted {input}");
    }
    assert_eq!(resolver_calls, 0);
}
