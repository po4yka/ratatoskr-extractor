//! Content-addressed artifact storage tests.

use bytes::Bytes;
use extractor_blob_store::BlobStore;
use extractor_test_support::TemporaryBlobRoot;
use futures_util::stream;
use ratatoskr_identifiers::DigestAlgorithm;

#[tokio::test]
async fn stored_artifact_is_announced_by_matching_blob_ref()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let bytes = Bytes::from_static(b"raw artifact bytes");

    let reference = store
        .store(
            "text/plain",
            stream::iter([Ok::<_, std::io::Error>(bytes.clone())]),
        )
        .await?;
    let path = store.resolve(&reference)?;
    let stored = tokio::fs::read(path).await?;

    assert_eq!(reference.owner_service.as_str(), "ratatoskr-extractor");
    assert_eq!(reference.digest.algorithm, DigestAlgorithm::Sha256);
    assert_eq!(reference.length_bytes, u64::try_from(bytes.len())?);
    assert_eq!(stored, bytes);
    assert_eq!(reference.digest.hex.as_str(), sha256_hex(&stored));

    let serialized = serde_json::to_string(&reference)?;
    assert!(!serialized.contains(root.path().to_string_lossy().as_ref()));
    assert!(!serialized.contains("http://"));
    assert!(!serialized.contains("https://"));
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    use sha2::{Digest as _, Sha256};

    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[tokio::test]
async fn duplicate_bytes_reuse_one_verified_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let bytes = Bytes::from_static(b"same artifact");
    let first_stream = stream::iter([Ok::<_, std::io::Error>(bytes.clone())]);
    let second_stream = stream::iter([Ok::<_, std::io::Error>(bytes.clone())]);

    let (first, second) = tokio::join!(
        store.store("application/octet-stream", first_stream),
        store.store("application/octet-stream", second_stream)
    );
    let first = first?;
    let second = second?;
    assert_eq!(first, second);
    let target = store.resolve(&first)?;
    assert_eq!(tokio::fs::read(&target).await?, bytes);

    tokio::fs::write(&target, b"different bytes").await?;
    let collision = store
        .store(
            "application/octet-stream",
            stream::iter([Ok::<_, std::io::Error>(bytes)]),
        )
        .await;
    assert!(collision.is_err());
    assert_eq!(tokio::fs::read(target).await?, b"different bytes");
    Ok(())
}

#[tokio::test]
async fn failed_stream_leaves_no_committed_or_staging_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let failed = stream::iter([
        Ok(Bytes::from_static(b"partial")),
        Err(std::io::Error::other("injected stream failure")),
    ]);

    assert!(store.store("text/plain", failed).await.is_err());
    assert!(!tokio::fs::try_exists(root.path().join("sha256")).await?);
    let staging = root.path().join("staging");
    assert_eq!(directory_entry_count(&staging).await?, 0);

    let stale = staging.join("ratatoskr-extractor-stale.part");
    let unrelated = staging.join("operator-note");
    tokio::fs::write(&stale, b"partial").await?;
    tokio::fs::write(&unrelated, b"keep").await?;
    store.prepare().await?;
    assert!(!tokio::fs::try_exists(stale).await?);
    assert!(tokio::fs::try_exists(unrelated).await?);
    Ok(())
}

async fn directory_entry_count(path: &std::path::Path) -> Result<usize, std::io::Error> {
    let mut entries = tokio::fs::read_dir(path).await?;
    let mut count = 0;
    while entries.next_entry().await?.is_some() {
        count += 1;
    }
    Ok(count)
}
