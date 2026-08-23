//! Service ownership of stored artifacts.

use extractor_blob_store::BlobStore;
use extractor_test_support::TemporaryBlobRoot;

#[tokio::test]
async fn worker_owned_store_publishes_worker_blobs() -> Result<(), Box<dyn std::error::Error>> {
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path()).with_owner("ratatoskr-browser-worker")?;
    let reference = store
        .store(
            "text/html",
            futures_util::stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from_static(
                b"<html></html>" as &[u8],
            ))]),
        )
        .await?;
    assert_eq!(
        reference.owner_service.as_str(),
        "ratatoskr-browser-worker",
        "the owning service names the deployable that stored the bytes"
    );
    store.verify(&reference).await?;
    Ok(())
}
