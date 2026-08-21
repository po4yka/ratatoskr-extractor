//! Minimized synthetic calibration corpus.

use extractor_document_ir::{DocumentIrError, HtmlDocumentInput, ParseLimits, from_html};
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

struct Case {
    name: &'static str,
    html: &'static [u8],
    expected_strategy: Option<&'static str>,
    score_range: std::ops::RangeInclusive<u16>,
}

#[test]
fn calibration_cases_keep_expected_winners_and_score_ranges()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        Case {
            name: "semantic",
            html: include_bytes!("fixtures/semantic.html"),
            expected_strategy: Some("semantic"),
            score_range: 780..=850,
        },
        Case {
            name: "noisy",
            html: include_bytes!("fixtures/noisy.html"),
            expected_strategy: Some("readability"),
            score_range: 900..=1000,
        },
        Case {
            name: "malformed",
            html: include_bytes!("fixtures/malformed.html"),
            expected_strategy: Some("readability"),
            score_range: 780..=820,
        },
        Case {
            name: "login",
            html: include_bytes!("fixtures/login.html"),
            expected_strategy: None,
            score_range: 0..=349,
        },
    ];
    for case in cases {
        assert_case(&case)?;
    }
    Ok(())
}

fn assert_case(case: &Case) -> Result<(), Box<dyn std::error::Error>> {
    let result = from_html(
        HtmlDocumentInput {
            document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000007")?,
            source_address: DocumentAddress::parse("https://example.com/corpus")?,
            source_blob: blob_ref(case.html.len())?,
            bytes: case.html,
        },
        ParseLimits {
            max_input_bytes: 4_096,
            max_dom_nodes: 128,
        },
    );
    match (case.expected_strategy, result) {
        (Some(expected), Ok(extraction)) => {
            let selected = extraction
                .candidates
                .iter()
                .find(|candidate| candidate.selected)
                .ok_or("accepted corpus case has no selected candidate")?;
            assert_eq!(selected.strategy, expected, "{} winner", case.name);
            assert!(
                case.score_range.contains(&selected.score),
                "{} score {} is outside {:?}",
                case.name,
                selected.score,
                case.score_range
            );
        }
        (None, Err(DocumentIrError::LowQuality { candidates })) => {
            assert!(candidates.iter().all(|candidate| !candidate.selected));
        }
        (expected, outcome) => {
            return Err(format!("{} expected {expected:?}, got {outcome:?}", case.name).into());
        }
    }
    Ok(())
}

fn blob_ref(length: usize) -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"e".repeat(64))?,
        },
        media_type: MediaType::parse("text/html")?,
        length_bytes: u64::try_from(length)?,
    })
}
