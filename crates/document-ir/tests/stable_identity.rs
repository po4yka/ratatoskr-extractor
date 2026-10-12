//! A document's identity is derived from its owner and its content digest, never minted per run
//! (XR-021 CONTRACTS.md R5-02 rules 1 and 2).

use extractor_document_ir::{assemble_document, paragraph_block, stable_document_id};
use ratatoskr_document_contracts::{DocumentAddress, ExtractionStrategy};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, MediaType, UserId,
};

const DIGEST_HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER_DIGEST_HEX: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const OWNER: &str = "018f0000-0000-7000-8000-000000000001";
const OTHER_OWNER: &str = "018f0000-0000-7000-8000-000000000002";

/// Produced once outside the code under test, with Python 3:
///
/// ```text
/// h = sha256(b"ratatoskr.document.v1\0" + UUID(OWNER).bytes + DIGEST_HEX.encode()).digest()[:16]
/// h[6] = (h[6] & 0x0f) | 0x80; h[8] = (h[8] & 0x3f) | 0x80
/// ```
const GOLDEN: &str = "668cde79-b0aa-8041-a227-7a48c2692ac9";

fn digest(hex: &str) -> Result<ContentDigest, Box<dyn std::error::Error>> {
    Ok(ContentDigest {
        algorithm: DigestAlgorithm::Sha256,
        hex: DigestHex::parse(hex)?,
    })
}

#[test]
fn stable_document_id_is_deterministic() -> Result<(), Box<dyn std::error::Error>> {
    let owner = UserId::parse(OWNER)?;
    assert_eq!(
        stable_document_id(owner, &digest(DIGEST_HEX)?),
        stable_document_id(owner, &digest(DIGEST_HEX)?)
    );
    Ok(())
}

#[test]
fn stable_document_id_differs_across_owners() -> Result<(), Box<dyn std::error::Error>> {
    let digest = digest(DIGEST_HEX)?;
    assert_ne!(
        stable_document_id(UserId::parse(OWNER)?, &digest),
        stable_document_id(UserId::parse(OTHER_OWNER)?, &digest)
    );
    Ok(())
}

#[test]
fn stable_document_id_differs_across_digests() -> Result<(), Box<dyn std::error::Error>> {
    let owner = UserId::parse(OWNER)?;
    assert_ne!(
        stable_document_id(owner, &digest(DIGEST_HEX)?),
        stable_document_id(owner, &digest(OTHER_DIGEST_HEX)?)
    );
    Ok(())
}

#[test]
fn stable_document_id_is_version_8_with_the_rfc_4122_variant()
-> Result<(), Box<dyn std::error::Error>> {
    let id = stable_document_id(UserId::parse(OWNER)?, &digest(DIGEST_HEX)?).0;
    assert_eq!(id.get_version_num(), 8);
    assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
    Ok(())
}

#[test]
fn stable_document_id_matches_the_golden_value() -> Result<(), Box<dyn std::error::Error>> {
    let id = stable_document_id(UserId::parse(OWNER)?, &digest(DIGEST_HEX)?);
    assert_eq!(id.0.to_string(), GOLDEN);
    Ok(())
}

#[test]
fn assemble_document_derives_the_identity_from_the_owner() -> Result<(), Box<dyn std::error::Error>>
{
    let first = assemble(OWNER, "Same text.")?;
    let again = assemble(OWNER, "Same text.")?;
    let other_owner = assemble(OTHER_OWNER, "Same text.")?;
    let other_text = assemble(OWNER, "Other text.")?;

    assert_eq!(first.document_id, again.document_id);
    assert_eq!(first.content_digest, again.content_digest);
    assert_ne!(first.document_id, other_owner.document_id);
    assert_ne!(first.document_id, other_text.document_id);
    assert_eq!(
        first.document_id,
        stable_document_id(UserId::parse(OWNER)?, &first.content_digest)
    );
    Ok(())
}

fn assemble(
    owner: &str,
    text: &str,
) -> Result<ratatoskr_document_contracts::Document, Box<dyn std::error::Error>> {
    let blob = BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: digest(DIGEST_HEX)?,
        media_type: MediaType::parse("text/html")?,
        length_bytes: 1,
    };
    Ok(assemble_document(
        UserId::parse(owner)?,
        DocumentAddress::parse("https://example.com/a")?,
        &blob,
        &ExtractionStrategy::parse("semantic_article")?,
        None,
        None,
        vec![paragraph_block(text.to_owned())],
    )?)
}
