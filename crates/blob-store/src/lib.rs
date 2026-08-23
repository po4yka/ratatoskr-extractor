#![forbid(unsafe_code)]

//! Content-addressed raw artifacts owned by Ratatoskr Extractor.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use futures_util::{Stream, StreamExt as _};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, MediaType,
};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const OWNER: &str = "ratatoskr-extractor";

/// Extractor-owned content-addressed raw artifact store.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
    owner: String,
}

/// Why a raw artifact could not be stored or verified.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BlobStoreError {
    /// Artifact persistence is not available.
    #[error("artifact persistence is unavailable")]
    Unavailable,
    /// Local artifact I/O failed.
    #[error("artifact I/O failed")]
    Io(#[from] std::io::Error),
    /// A body stream failed before it completed.
    #[error("the artifact stream failed")]
    Stream,
    /// Artifact length cannot fit the shared contract.
    #[error("the artifact length exceeds the supported range")]
    LengthOverflow,
    /// The effective media type does not match the shared contract.
    #[error("the artifact media type is invalid")]
    InvalidMediaType,
    /// A service-controlled contract value is invalid.
    #[error("the artifact identity could not be constructed")]
    InvalidIdentity,
    /// Existing content at a digest path does not match its digest.
    #[error("the artifact digest path contains different bytes")]
    Collision,
    /// The referenced artifact is not present.
    #[error("the referenced artifact is missing")]
    Missing,
    /// Stored bytes do not match the reference.
    #[error("the referenced artifact failed integrity verification")]
    Mismatch,
    /// The reference is owned by another service.
    #[error("the artifact reference has the wrong owner")]
    WrongOwner,
    /// The reference uses an unsupported digest algorithm.
    #[error("the artifact digest algorithm is unsupported")]
    UnsupportedAlgorithm,
}

impl BlobStore {
    /// Uses `root` as the private content-addressed store.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            owner: OWNER.to_owned(),
        }
    }

    /// Names the owning service recorded in every published [`BlobRef`].
    ///
    /// # Errors
    ///
    /// Returns [`BlobStoreError`] when the owner name violates the shared contract.
    pub fn with_owner(mut self, owner: &str) -> Result<Self, BlobStoreError> {
        BlobOwner::parse(owner).map_err(|_| BlobStoreError::InvalidIdentity)?;
        owner.clone_into(&mut self.owner);
        Ok(self)
    }

    /// Prepares store directories before the service becomes ready.
    ///
    /// # Errors
    ///
    /// Returns [`BlobStoreError`] when the store root cannot be prepared.
    pub async fn prepare(&self) -> Result<(), BlobStoreError> {
        let staging = self.root.join("staging");
        tokio::fs::create_dir_all(&staging).await?;
        let mut entries = tokio::fs::read_dir(staging).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("ratatoskr-extractor-") && name.ends_with(".part") {
                tokio::fs::remove_file(entry.path()).await?;
            }
        }
        Ok(())
    }

    /// Streams one raw artifact into the store.
    ///
    /// # Errors
    ///
    /// Returns [`BlobStoreError`] for a stream or persistence failure.
    pub async fn store<S, E>(
        &self,
        media_type: &str,
        mut stream: S,
    ) -> Result<BlobRef, BlobStoreError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::error::Error,
    {
        let owner = BlobOwner::parse(&self.owner).map_err(|_| BlobStoreError::InvalidIdentity)?;
        let media_type =
            MediaType::parse(media_type).map_err(|_| BlobStoreError::InvalidMediaType)?;
        let staging_directory = self.root.join("staging");
        tokio::fs::create_dir_all(&staging_directory).await?;
        let staging_path =
            staging_directory.join(format!("ratatoskr-extractor-{}.part", uuid::Uuid::now_v7()));
        let mut file = tokio::fs::File::create(&staging_path).await?;
        let mut hasher = Sha256::new();
        let mut length = 0_u64;

        while let Some(item) = stream.next().await {
            let Ok(chunk) = item else {
                drop(file);
                let _ = tokio::fs::remove_file(&staging_path).await;
                return Err(BlobStoreError::Stream);
            };
            let chunk_length =
                u64::try_from(chunk.len()).map_err(|_| BlobStoreError::LengthOverflow)?;
            length = length
                .checked_add(chunk_length)
                .ok_or(BlobStoreError::LengthOverflow)?;
            if let Err(error) = file.write_all(&chunk).await {
                drop(file);
                let _ = tokio::fs::remove_file(&staging_path).await;
                return Err(BlobStoreError::Io(error));
            }
            hasher.update(&chunk);
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        let digest_bytes = hasher.finalize();
        let digest_text = digest_hex(&digest_bytes);
        let digest = DigestHex::parse(&digest_text).map_err(|_| BlobStoreError::InvalidIdentity)?;
        let target = self.path_for_digest(&digest_text)?;
        let parent = target
            .parent()
            .ok_or_else(|| std::io::Error::other("artifact target has no parent"))?;
        tokio::fs::create_dir_all(parent).await?;
        match tokio::fs::hard_link(&staging_path, &target).await {
            Ok(()) => tokio::fs::remove_file(&staging_path).await?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let verified = verify_file(&target, &digest_text, length).await;
                let _ = tokio::fs::remove_file(&staging_path).await;
                verified?;
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&staging_path).await;
                return Err(BlobStoreError::Io(error));
            }
        }
        Ok(BlobRef {
            owner_service: owner,
            digest: ContentDigest {
                algorithm: DigestAlgorithm::Sha256,
                hex: digest,
            },
            media_type,
            length_bytes: length,
        })
    }

    /// Resolves a reference to an internal path without exposing it in the contract.
    ///
    /// # Errors
    ///
    /// Returns [`BlobStoreError`] when the reference digest cannot form a store path.
    pub fn resolve(&self, reference: &BlobRef) -> Result<PathBuf, BlobStoreError> {
        self.path_for_digest(reference.digest.hex.as_str())
    }

    /// Verifies a prior reference before cached bytes are reused.
    ///
    /// # Errors
    ///
    /// Returns [`BlobStoreError`] when ownership or stored bytes do not match the reference.
    pub async fn verify(&self, reference: &BlobRef) -> Result<PathBuf, BlobStoreError> {
        if reference.owner_service.as_str() != self.owner.as_str() {
            return Err(BlobStoreError::WrongOwner);
        }
        match reference.digest.algorithm {
            DigestAlgorithm::Sha256 => {}
            _ => return Err(BlobStoreError::UnsupportedAlgorithm),
        }
        MediaType::parse(reference.media_type.as_str()).map_err(|_| BlobStoreError::Mismatch)?;
        let path = self.resolve(reference)?;
        match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(BlobStoreError::Mismatch),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(BlobStoreError::Missing);
            }
            Err(error) => return Err(BlobStoreError::Io(error)),
        }
        match verify_file(&path, reference.digest.hex.as_str(), reference.length_bytes).await {
            Ok(()) => Ok(path),
            Err(BlobStoreError::Collision) => Err(BlobStoreError::Mismatch),
            Err(error) => Err(error),
        }
    }

    fn path_for_digest(&self, digest: &str) -> Result<PathBuf, BlobStoreError> {
        let prefix = digest
            .get(..2)
            .ok_or_else(|| std::io::Error::other("digest prefix is missing"))?;
        let suffix = digest
            .get(2..)
            .ok_or_else(|| std::io::Error::other("digest suffix is missing"))?;
        Ok(self.root.join("sha256").join(prefix).join(suffix))
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

async fn verify_file(
    path: &Path,
    expected_digest: &str,
    expected_length: u64,
) -> Result<(), BlobStoreError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = [0_u8; 8_192];
    let mut hasher = Sha256::new();
    let mut length = 0_u64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let read_length = u64::try_from(read).map_err(|_| BlobStoreError::LengthOverflow)?;
        length = length
            .checked_add(read_length)
            .ok_or(BlobStoreError::LengthOverflow)?;
        let bytes = buffer.get(..read).ok_or(BlobStoreError::Collision)?;
        hasher.update(bytes);
    }
    let actual_digest = digest_hex(&hasher.finalize());
    if length == expected_length && actual_digest == expected_digest {
        Ok(())
    } else {
        Err(BlobStoreError::Collision)
    }
}
