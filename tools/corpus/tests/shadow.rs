//! Offline legacy-shadow comparison contract.

use extractor_corpus::shadow::{render_report, verify_report_at};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn shadow_report_keeps_source_classes_independent() {
    let report = render_report().expect("committed shadow samples must render");

    assert!(report.contains("## web_article: approve"));
    assert!(report.contains("## youtube_transcript: approve"));
    assert!(report.contains("## x_post: hold"));
    assert!(report.contains("current extraction is unsupported: x-post"));
}

#[test]
fn shadow_report_matches_committed_review_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "ratatoskr-shadow-review-{}-{nonce}",
        std::process::id()
    ));
    let shadow = root.join("shadow");
    fs::create_dir_all(&shadow)?;
    fs::copy(
        extractor_corpus::corpus_root().join("shadow/cases.json"),
        shadow.join("cases.json"),
    )?;
    let expected_path = shadow.join("report.md");
    fs::copy(
        extractor_corpus::corpus_root().join("shadow/report.md"),
        &expected_path,
    )?;

    verify_report_at(&root)?;
    let original = fs::read_to_string(&expected_path)?;
    fs::write(&expected_path, "changed report")?;
    assert!(verify_report_at(&root).is_err());
    assert_eq!(fs::read_to_string(&expected_path)?, "changed report");
    fs::write(&expected_path, original)?;

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn shadow_report_withholds_approval_for_coverage_regression()
-> Result<(), Box<dyn std::error::Error>> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "ratatoskr-shadow-coverage-{}-{nonce}",
        std::process::id()
    ));
    let shadow = root.join("shadow");
    fs::create_dir_all(&shadow)?;
    let source = fs::read_to_string(extractor_corpus::corpus_root().join("shadow/cases.json"))?;
    let changed = source.replace("\"minimum_overlap\": 0.90", "\"minimum_overlap\": 1.01");
    fs::write(shadow.join("cases.json"), changed)?;

    let report = extractor_corpus::shadow::render_report_at(&root)?;

    assert!(report.contains("## web_article: hold"));
    assert!(report.contains("content overlap below 1.010: web-semantic-article (1.000)"));

    fs::remove_dir_all(root)?;
    Ok(())
}
