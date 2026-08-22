//! Shared plain-text evaluation feeding non-DOM extraction paths.

use extractor_document_ir::{QualityReason, evaluate_plain_text};
use ratatoskr_document_contracts::DocumentBlock;

#[test]
fn plain_text_candidate_reuses_shared_thresholds() {
    let blocks = vec![DocumentBlock::Paragraph {
        text: "ab".repeat(150),
    }];
    let decision = evaluate_plain_text("direct_pdf", &blocks, Some("Direct Extraction Fixture"));

    assert_eq!(decision.strategy, "direct_pdf");
    assert_eq!(decision.evaluator_version, "quality_v1");
    assert_eq!(decision.blocks, blocks);
    assert_eq!(decision.metrics.text_characters, 300);
    assert_eq!(decision.metrics.paragraph_count, 1);
    assert_eq!(decision.metrics.text_volume, 300);
    assert_eq!(decision.metrics.paragraph_distribution, 50);
    assert_eq!(decision.metrics.non_link_share, 200);
    assert_eq!(decision.metrics.non_boilerplate_share, 200);
    assert_eq!(decision.metrics.title_agreement, 0);
    assert_eq!(decision.score, 750);
    assert!(decision.accepted);
    assert_eq!(decision.reasons, vec![QualityReason::Accepted]);
    assert!(!decision.selected);
}

#[test]
fn plain_text_below_volume_threshold_is_too_short_only() {
    let blocks = vec![DocumentBlock::Paragraph {
        text: "ab".repeat(50),
    }];
    let decision = evaluate_plain_text("direct_pdf", &blocks, None);

    assert_eq!(decision.metrics.text_characters, 100);
    assert_eq!(decision.score, 550);
    assert!(!decision.accepted);
    assert_eq!(decision.reasons, vec![QualityReason::TooShort]);
}
