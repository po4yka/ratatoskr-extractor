//! Blessing remains a narrow, explicit write action.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use extractor_corpus::{bless_case_at, verify_case_at};

#[test]
fn bless_updates_only_the_named_case() -> Result<(), Box<dyn std::error::Error>> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let root = std::env::temp_dir().join(format!(
        "ratatoskr-corpus-bless-{}-{nonce}",
        std::process::id()
    ));
    let expected = root.join("expected");
    fs::create_dir_all(&expected)?;
    let unrelated = expected.join("unrelated.json");
    fs::write(&unrelated, "unchanged")?;

    let written = bless_case_at(&root, "html-semantic")?;
    assert_eq!(written, expected.join("html-semantic.json"));
    assert_eq!(fs::read_to_string(&unrelated)?, "unchanged");
    verify_case_at(&root, "html-semantic")?;

    fs::remove_dir_all(&root)?;
    Ok(())
}
