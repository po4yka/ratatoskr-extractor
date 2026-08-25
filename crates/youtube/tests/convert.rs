//! Deterministic transcript-to-Document conversion behavior.

use extractor_youtube::sidecar::BlockTiming;
use extractor_youtube::{
    PlayerMeta, Segment, TranscriptInput, YOUTUBE_STRATEGY, YoutubeError, YoutubeLimits,
    from_transcript,
};
use ratatoskr_document_contracts::{DocumentAddress, DocumentBlock, LanguageTag};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

const DOCUMENT_ID: &str = "018f0000-0000-7000-8000-000000000042";
const SOURCE_ADDRESS: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

fn blob_ref(length: usize) -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"ab".repeat(32))?,
        },
        media_type: MediaType::parse("text/html")?,
        length_bytes: u64::try_from(length)?,
    })
}

fn segment(start_ms: u64, duration_ms: u64, text: &str) -> Segment {
    Segment {
        start_ms,
        duration_ms,
        text: text.to_owned(),
    }
}

fn limits(max_blocks: usize) -> YoutubeLimits {
    YoutubeLimits {
        max_page_bytes: 65_536,
        max_track_bytes: 65_536,
        max_blocks,
        max_segments: 10_000,
        max_segment_characters: 1_000,
    }
}

fn meta() -> PlayerMeta {
    PlayerMeta {
        title: Some("Test Video".to_owned()),
        channel: Some("Example Channel".to_owned()),
        duration_seconds: Some(2123),
        tracks: vec![],
    }
}

fn input<'a>(
    meta: &'a PlayerMeta,
    segments: Vec<Segment>,
    blob: &BlobRef,
) -> Result<TranscriptInput<'a>, Box<dyn std::error::Error>> {
    input_in(meta, segments, blob, "en")
}

fn input_in<'a>(
    meta: &'a PlayerMeta,
    segments: Vec<Segment>,
    blob: &BlobRef,
    language: &'a str,
) -> Result<TranscriptInput<'a>, Box<dyn std::error::Error>> {
    Ok(TranscriptInput {
        document_id: DocumentId::parse(DOCUMENT_ID)?,
        source_address: DocumentAddress::parse(SOURCE_ADDRESS)?,
        source_blob: blob.clone(),
        meta,
        segments,
        language,
    })
}

#[test]
fn segments_within_the_block_budget_become_one_paragraph_per_segment()
-> Result<(), Box<dyn std::error::Error>> {
    let blob = blob_ref(4096)?;
    let segments = vec![
        segment(0, 4_000, "First line."),
        segment(4_000, 3_500, "Second line."),
        segment(7_500, 2_500, "Third line."),
    ];
    let extraction = from_transcript(input(&meta(), segments.clone(), &blob)?, limits(5))?;

    assert_eq!(
        extraction.document.blocks,
        vec![
            DocumentBlock::Paragraph {
                text: "First line.".to_owned()
            },
            DocumentBlock::Paragraph {
                text: "Second line.".to_owned()
            },
            DocumentBlock::Paragraph {
                text: "Third line.".to_owned()
            },
        ]
    );

    let sidecar_blocks = &extraction.sidecar.blocks;
    assert_eq!(sidecar_blocks.len(), 3);
    for (index, (block, seg)) in sidecar_blocks.iter().zip(segments.iter()).enumerate() {
        assert_eq!(block.block_index, u32::try_from(index)?);
        assert_eq!(block.start_ms, seg.start_ms);
        assert_eq!(block.end_ms, seg.start_ms + seg.duration_ms);
    }
    assert_eq!(extraction.sidecar.segment_count, 3);
    Ok(())
}

#[test]
fn segments_past_the_block_budget_merge_into_deterministic_contiguous_groups()
-> Result<(), Box<dyn std::error::Error>> {
    let blob = blob_ref(4096)?;
    let segments = vec![
        segment(0, 1_000, "one"),
        segment(1_000, 1_000, "two"),
        segment(2_000, 1_000, "three"),
        segment(3_000, 1_000, "four"),
        segment(4_000, 1_500, "five"),
    ];
    let extraction = from_transcript(input(&meta(), segments, &blob)?, limits(2))?;

    // floor(i * n / G): group 0 covers [0, 2), group 1 covers [2, 5).
    assert_eq!(extraction.document.blocks.len(), 2);
    assert_eq!(
        extraction.document.blocks,
        vec![
            DocumentBlock::Paragraph {
                text: "one two".to_owned()
            },
            DocumentBlock::Paragraph {
                text: "three four five".to_owned()
            },
        ]
    );
    assert_eq!(extraction.sidecar.segment_count, 5);
    assert_eq!(
        extraction.sidecar.blocks,
        vec![
            BlockTiming {
                block_index: 0,
                start_ms: 0,
                end_ms: 2_000
            },
            BlockTiming {
                block_index: 1,
                start_ms: 2_000,
                end_ms: 5_500
            },
        ]
    );
    Ok(())
}

#[test]
fn repeated_conversion_is_byte_identical_in_document_digest_and_sidecar()
-> Result<(), Box<dyn std::error::Error>> {
    let blob = blob_ref(4096)?;
    let segments = vec![
        segment(0, 1_000, "one"),
        segment(1_000, 1_000, "two"),
        segment(2_000, 1_000, "three"),
        segment(3_000, 1_000, "four"),
        segment(4_000, 1_500, "five"),
    ];
    let first = from_transcript(input(&meta(), segments.clone(), &blob)?, limits(2))?;
    let second = from_transcript(input(&meta(), segments, &blob)?, limits(2))?;

    assert_eq!(first.document, second.document);
    assert_eq!(
        first.document.content_digest,
        second.document.content_digest
    );
    assert_eq!(first.sidecar, second.sidecar);
    Ok(())
}

#[test]
fn title_language_provenance_and_candidate_follow_the_input()
-> Result<(), Box<dyn std::error::Error>> {
    let blob = blob_ref(4096)?;
    let padded = PlayerMeta {
        title: Some("  Test Video  ".to_owned()),
        ..meta()
    };
    let extraction = from_transcript(
        input_in(&padded, vec![segment(0, 1_000, "only line")], &blob, "en")?,
        limits(4),
    )?;

    assert_eq!(extraction.document.title.as_deref(), Some("Test Video"));
    assert_eq!(extraction.document.language, LanguageTag::parse("en").ok());

    let provenance = &extraction.document.provenance;
    assert_eq!(provenance.len(), extraction.document.blocks.len());
    for (index, entry) in provenance.iter().enumerate() {
        assert_eq!(entry.block_index, u32::try_from(index)?);
        assert_eq!(entry.extraction_strategy.as_str(), YOUTUBE_STRATEGY);
        assert_eq!(entry.source_blob, blob);
    }

    assert!(
        extraction.candidate.selected,
        "transcript candidate is authoritative"
    );
    assert_eq!(extraction.candidate.strategy, YOUTUBE_STRATEGY);
    assert_eq!(extraction.candidate.blocks.len(), 1);
    Ok(())
}

#[test]
fn an_empty_segment_list_is_a_schema_failure() -> Result<(), Box<dyn std::error::Error>> {
    let blob = blob_ref(4096)?;
    let error = from_transcript(input(&meta(), vec![], &blob)?, limits(8))
        .expect_err("empty segments must be refused");
    assert!(matches!(error, YoutubeError::Schema));
    Ok(())
}

#[test]
fn an_invalid_language_tag_is_an_identity_failure() -> Result<(), Box<dyn std::error::Error>> {
    let blob = blob_ref(4096)?;
    let error = from_transcript(
        input_in(&meta(), vec![segment(0, 100, "text")], &blob, "not a tag!")?,
        limits(8),
    )
    .expect_err("invalid language tags must be refused");
    assert!(matches!(error, YoutubeError::InvalidIdentity));
    Ok(())
}

#[test]
fn a_zero_block_budget_is_a_resource_limit_failure() -> Result<(), Box<dyn std::error::Error>> {
    let blob = blob_ref(4096)?;
    let error = from_transcript(
        input(&meta(), vec![segment(0, 100, "only")], &blob)?,
        limits(0),
    )
    .expect_err("a zero block budget must be refused");
    assert!(matches!(error, YoutubeError::ResourceLimit));
    Ok(())
}
