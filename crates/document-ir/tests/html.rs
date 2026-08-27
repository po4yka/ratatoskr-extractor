//! Public HTML-to-Document-IR behavior.

use extractor_document_ir::{DocumentIrError, HtmlDocumentInput, ParseLimits, from_html};
use ratatoskr_document_contracts::{DocumentAddress, DocumentBlock};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

#[test]
fn one_dom_produces_ordered_shared_blocks_and_provenance() -> Result<(), Box<dyn std::error::Error>>
{
    let source = blob_ref()?;
    let document = from_html(
        HtmlDocumentInput {
            document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000002")?,
            source_address: DocumentAddress::parse("https://example.com/article")?,
            source_blob: source.clone(),
            bytes: br#"<!doctype html><html lang="en"><head><title>  Ratatoskr   Notes </title></head><body><h1> First </h1><p>Hello world. This paragraph records enough verified detail to remain useful after deterministic quality evaluation.</p><h2>Second</h2><p>More text follows with concrete context, complete sentences, and evidence for every downstream reader.</p></body></html>"#,
        },
        ParseLimits {
            max_input_bytes: 4_096,
            max_dom_nodes: 64,
        },
    )?
    .document;

    assert_eq!(document.title.as_deref(), Some("Ratatoskr Notes"));
    assert_eq!(
        block_texts(&document.blocks),
        [
            "First",
            "Hello world. This paragraph records enough verified detail to remain useful after deterministic quality evaluation.",
            "Second",
            "More text follows with concrete context, complete sentences, and evidence for every downstream reader.",
        ]
    );
    assert_eq!(document.provenance.len(), document.blocks.len());
    assert!(
        document
            .provenance
            .iter()
            .enumerate()
            .all(|(index, evidence)| u32::try_from(index)
                .is_ok_and(|block_index| evidence.block_index == block_index)
                && evidence.source_blob == source
                && evidence.extraction_strategy.as_str() == "readability")
    );

    assert!(document.blocks.iter().all(|block| match block {
        DocumentBlock::Heading { block_id, .. } | DocumentBlock::Paragraph { block_id, .. } => {
            !block_id.to_string().is_empty()
        }
        _ => false,
    }));
    Ok(())
}

fn block_texts(blocks: &[DocumentBlock]) -> Vec<&str> {
    blocks
        .iter()
        .map(|block| match block {
            DocumentBlock::Heading { text, .. } | DocumentBlock::Paragraph { text, .. } => {
                text.as_str()
            }
            _ => "",
        })
        .collect()
}

#[test]
fn malformed_html_is_recovered_deterministically() -> Result<(), Box<dyn std::error::Error>> {
    let source = blob_ref()?;
    let document_id = DocumentId::parse("018f0000-0000-7000-8000-000000000004")?;
    let source_address = DocumentAddress::parse("https://example.com/malformed")?;
    let convert = || {
        from_html(
            HtmlDocumentInput {
                document_id,
                source_address: source_address.clone(),
                source_blob: source.clone(),
                bytes: b"<title>Broken<title><body><h1>Heading<p>first<h2>Next<p>second",
            },
            ParseLimits {
                max_input_bytes: 4_096,
                max_dom_nodes: 64,
            },
        )
    };

    match (convert(), convert()) {
        (
            Err(DocumentIrError::LowQuality { candidates: first }),
            Err(DocumentIrError::LowQuality { candidates: second }),
        ) => {
            assert_eq!(first, second);
            Ok(())
        }
        (first, second) => {
            Err(format!("unexpected malformed results: {first:?}, {second:?}").into())
        }
    }
}

#[test]
fn document_ir_blocks_receive_unique_identifiers() -> Result<(), Box<dyn std::error::Error>> {
    let source = blob_ref()?;
    let document = from_html(
        HtmlDocumentInput {
            document_id: DocumentId::parse("018f0000-0000-7000-8000-000000000003")?,
            source_address: DocumentAddress::parse("https://example.com/identified")?,
            source_blob: source,
            bytes: br"<article><h1>Heading</h1><p>This paragraph contains enough concrete evidence to satisfy the deterministic quality evaluator for this test, including a second sentence with provenance-relevant details and a third sentence that makes the normalized content comfortably exceed the minimum text threshold.</p></article>",
        },
        ParseLimits {
            max_input_bytes: 4_096,
            max_dom_nodes: 64,
        },
    )?
    .document;
    let wire = serde_json::to_value(document)?;
    let blocks = wire
        .get("blocks")
        .and_then(serde_json::Value::as_array)
        .expect("Document IR serializes blocks as an array");
    let identifiers = blocks
        .iter()
        .map(|block| {
            block
                .get("block_id")
                .and_then(serde_json::Value::as_str)
                .expect("every emitted Document IR block has a serialized identifier")
        })
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(identifiers.len(), blocks.len());
    Ok(())
}

fn blob_ref() -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )?,
        },
        media_type: MediaType::parse("text/html")?,
        length_bytes: 168,
    })
}
