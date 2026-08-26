#![no_main]

use extractor_document_ir::{HtmlDocumentInput, ParseLimits, from_html};
use libfuzzer_sys::fuzz_target;
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

fuzz_target!(|data: &[u8]| {
    let Ok(document_id) = DocumentId::parse("018f0000-0000-7000-8000-000000000901") else {
        return;
    };
    let Ok(source_address) = DocumentAddress::parse("https://corpus.example/fuzz-html") else {
        return;
    };
    let Ok(source_blob) = blob(data.len(), "text/html") else {
        return;
    };
    let _ = from_html(
        HtmlDocumentInput {
            document_id,
            source_address,
            source_blob,
            bytes: data,
        },
        ParseLimits {
            max_input_bytes: 65_536,
            max_dom_nodes: 4_096,
        },
    );
});

fn blob(length: usize, media_type: &str) -> Result<BlobRef, ()> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor").map_err(|_| ())?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"ab".repeat(32)).map_err(|_| ())?,
        },
        media_type: MediaType::parse(media_type).map_err(|_| ())?,
        length_bytes: u64::try_from(length).map_err(|_| ())?,
    })
}
