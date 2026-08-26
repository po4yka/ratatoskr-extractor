//! Golden-corpus verification contract.

use extractor_corpus::verify;

#[test]
fn golden_corpus_verifies_committed_outputs() {
    assert!(verify().is_ok(), "every committed corpus case must verify");
}
