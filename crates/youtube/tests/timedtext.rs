//! Timed-text parser tests for both wire formats and every bounded failure mode.

use extractor_youtube::{Segment, YoutubeLimits, parse_timedtext};

fn limits() -> YoutubeLimits {
    YoutubeLimits {
        max_page_bytes: 65_536,
        max_track_bytes: 4_096,
        max_blocks: 2_000,
        max_segments: 16,
        max_segment_characters: 64,
    }
}

fn xml(payload: &str) -> Result<Vec<Segment>, extractor_youtube::YoutubeError> {
    parse_timedtext(payload.as_bytes(), &limits())
}

#[test]
fn xml_seconds_payload_maps_to_millisecond_segments() {
    let segments = xml(
        "<transcript><text start=\"6.52\" dur=\"2.48\">Hello &amp; welcome</text>\
         <text start=\"9.0\" dur=\"1.5\">Second line</text></transcript>",
    )
    .expect("XML payload parses");
    assert_eq!(
        segments,
        vec![
            Segment {
                start_ms: 6_520,
                duration_ms: 2_480,
                text: "Hello & welcome".to_owned(),
            },
            Segment {
                start_ms: 9_000,
                duration_ms: 1_500,
                text: "Second line".to_owned(),
            },
        ]
    );
}

#[test]
fn xml_millisecond_attributes_map_directly() {
    let segments =
        xml("<timedtext><body><p t=\"6520\" d=\"2480\">Plain text</p></body></timedtext>")
            .expect("XML payload parses");
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].start_ms, 6_520);
    assert_eq!(segments[0].duration_ms, 2_480);
    assert_eq!(segments[0].text, "Plain text");
}

#[test]
fn json3_events_concatenate_their_segment_texts() {
    let payload = r#"{"events":[
        {"tStartMs":6520,"dDurationMs":2480,"segs":[{"utf8":"Hello "},{"utf8":"world"}]},
        {"tStartMs":9000,"dDurationMs":1500,"segs":[{"utf8":"Two"}]}
    ]}"#;
    let segments = parse_timedtext(payload.as_bytes(), &limits()).expect("JSON3 parses");
    assert_eq!(
        segments,
        vec![
            Segment {
                start_ms: 6_520,
                duration_ms: 2_480,
                text: "Hello world".to_owned(),
            },
            Segment {
                start_ms: 9_000,
                duration_ms: 1_500,
                text: "Two".to_owned(),
            },
        ]
    );
}

#[test]
fn named_decimal_and_hex_entities_decode() {
    let segments = xml(
        "<transcript><text start=\"0\" dur=\"1\">&lt;a&gt; &amp; &#39;b&#x27; &quot;c&quot;</text></transcript>",
    )
    .expect("XML payload parses");
    assert_eq!(segments[0].text, "<a> & 'b' \"c\"");
}

#[test]
fn unknown_entities_pass_through_literally() {
    let segments = xml("<transcript><text start=\"0\" dur=\"1\">a &nosuch; b</text></transcript>")
        .expect("XML payload parses");
    assert_eq!(segments[0].text, "a &nosuch; b");
}

#[test]
fn oversized_payload_is_a_resource_limit() {
    let padded = format!("<transcript>{}</transcript>", " ".repeat(5_000));
    let error =
        parse_timedtext(padded.as_bytes(), &limits()).expect_err("oversized payload refused");
    assert!(matches!(
        error,
        extractor_youtube::YoutubeError::ResourceLimit
    ));
}

#[test]
fn segment_count_budget_is_enforced() {
    let mut entries = String::new();
    for index in 0..32 {
        let cue = format!("<text start=\"{index}\" dur=\"1\">t</text>");
        entries.push_str(&cue);
    }
    let payload = format!("<transcript>{entries}</transcript>");
    let error = parse_timedtext(payload.as_bytes(), &limits())
        .expect_err("segment-count overrun is refused");
    assert!(matches!(
        error,
        extractor_youtube::YoutubeError::ResourceLimit
    ));
}

#[test]
fn per_segment_text_budget_is_enforced() {
    let long_text = "w".repeat(200);
    let payload =
        format!("<transcript><text start=\"0\" dur=\"1\">{long_text}</text></transcript>");
    let error =
        parse_timedtext(payload.as_bytes(), &limits()).expect_err("long segment text refused");
    assert!(matches!(
        error,
        extractor_youtube::YoutubeError::ResourceLimit
    ));
}

#[test]
fn empty_event_lists_are_schema_errors() {
    for payload in [
        "<transcript></transcript>",
        "{\"events\":[]}",
        "{\"events\":[{\"tStartMs\":0,\"dDurationMs\":10}]}",
    ] {
        let error = parse_timedtext(payload.as_bytes(), &limits())
            .expect_err("payload without text nodes refused");
        assert!(
            matches!(error, extractor_youtube::YoutubeError::Schema),
            "{payload}"
        );
    }
}

#[test]
fn unrecognized_first_byte_is_a_schema_error() {
    let error = parse_timedtext(b"garbage", &limits()).expect_err("unrecognized payload refused");
    assert!(matches!(error, extractor_youtube::YoutubeError::Schema));
}

#[test]
fn truncated_json_is_a_schema_error_not_a_panic() {
    let error = parse_timedtext(br#"{"events":[{"tStartMs"#, &limits())
        .expect_err("truncated JSON refused");
    assert!(matches!(error, extractor_youtube::YoutubeError::Schema));
}

#[test]
fn negative_timing_is_a_schema_error() {
    let payload = r#"{"events":[{"tStartMs":-50,"dDurationMs":10,"segs":[{"utf8":"x"}]}]}"#;
    let error =
        parse_timedtext(payload.as_bytes(), &limits()).expect_err("negative timing refused");
    assert!(matches!(error, extractor_youtube::YoutubeError::Schema));
}

#[test]
fn parsing_twice_yields_identical_segments() {
    let payload = "<transcript><text start=\"1.5\" dur=\"2\">same</text></transcript>";
    let first = xml(payload).expect("XML payload parses");
    let second = xml(payload).expect("XML payload parses");
    assert_eq!(first, second);
}
