//! Cached artifact integrity tests.

use bytes::Bytes;
use extractor_blob_store::{BlobStore, BlobStoreError};
use extractor_test_support::TemporaryBlobRoot;
use futures_util::stream;

#[tokio::test]
async fn missing_or_changed_cached_bytes_fail_verification()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let reference = store
        .store(
            "text/plain",
            stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(
                b"verified bytes",
            ))]),
        )
        .await?;
    let path = store.resolve(&reference)?;

    tokio::fs::remove_file(&path).await?;
    assert!(matches!(
        store.verify(&reference).await,
        Err(BlobStoreError::Missing)
    ));

    tokio::fs::write(&path, b"changed bytes!").await?;
    assert!(matches!(
        store.verify(&reference).await,
        Err(BlobStoreError::Mismatch)
    ));
    Ok(())
}
