#![no_main]

use extractor_pdf::{PdfDocumentInput, PdfParseLimits, from_pdf};
use libfuzzer_sys::fuzz_target;
use ratatoskr_document_contracts::DocumentAddress;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
};

fuzz_target!(|data: &[u8]| {
    let Ok(document_id) = DocumentId::parse("018f0000-0000-7000-8000-000000000902") else {
        return;
    };
    let Ok(source_address) = DocumentAddress::parse("https://corpus.example/fuzz.pdf") else {
        return;
    };
    let Ok(source_blob) = blob(data.len()) else {
        return;
    };
    let _ = from_pdf(
        PdfDocumentInput {
            document_id,
            source_address,
            source_blob,
            bytes: data,
        },
        PdfParseLimits {
            max_input_bytes: 65_536,
            max_pages: 64,
            max_text_bytes: 1_048_576,
        },
    );
});

fn blob(length: usize) -> Result<BlobRef, ()> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor").map_err(|_| ())?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"cd".repeat(32)).map_err(|_| ())?,
        },
        media_type: MediaType::parse("application/pdf").map_err(|_| ())?,
        length_bytes: u64::try_from(length).map_err(|_| ())?,
    })
}
