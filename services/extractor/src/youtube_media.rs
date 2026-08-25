//! Gated, bounded `YouTube` media archival.
//!
//! Archival never influences extraction outcomes: every failure or skip becomes a recorded
//! diagnostic class. Byte budgets are enforced twice - inside the downloader contract while the
//! file materializes, and again while streaming into the content-addressed store - so an
//! oversized item can never land whole in memory or on disk under extractor control.

use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use extractor_blob_store::BlobStore;
use extractor_core::YoutubeConfig;
use extractor_persistence::reserve_media_archive;
use extractor_persistence::unexpired_media_bytes;

/// Why one archival attempt ended as it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchivalOutcome {
    /// The configuration gate is off; nothing was attempted.
    Disabled,
    /// An unexpired archive for this video already exists.
    SkippedDuplicate,
    /// The persisted total-byte budget is consumed.
    SkippedBudgetExhausted,
    /// A concurrent reservation consumed the remaining budget after download.
    SkippedBudgetRace,
    /// Bytes stored and both accounting rows committed.
    Stored {
        /// Digest hex of the stored artifact.
        digest_hex: String,
        /// Exact byte length recorded with the artifact.
        length_bytes: u64,
    },
    /// Download failed; the class records why.
    FailedDownload {
        /// Stable bounded failure class.
        class: &'static str,
    },
}

/// One downloaded media file within the per-item cap.
#[derive(Debug)]
pub struct DownloadedMedia {
    /// Path of the completed file inside the caller-owned working directory.
    pub path: PathBuf,
    /// Exact byte length of the file.
    pub length_bytes: u64,
}

/// Why one download attempt failed.
#[derive(Debug, thiserror::Error)]
#[error("media download failed ({class})")]
pub struct MediaDownloadError {
    /// Stable bounded failure class safe for diagnostics.
    pub class: &'static str,
}

/// Acquires one video's media file under hard caps.
pub trait MediaDownloader: Send + Sync {
    /// Downloads media for `video_url` into `working_dir`, aborting once `max_item_bytes` would
    /// be exceeded.
    ///
    /// # Errors
    ///
    /// Returns [`MediaDownloadError`] with a stable class for timeouts, nonzero exits, oversized
    /// items, and IO failures.
    fn download(
        &self,
        video_url: &str,
        max_item_bytes: u64,
        working_dir: &Path,
    ) -> impl Future<Output = Result<DownloadedMedia, MediaDownloadError>> + Send;
}

/// Drives one archival attempt against persistence, blob storage, and a downloader.
#[derive(Debug)]
pub struct MediaArchiver<'a, D> {
    pool: &'a sqlx::PgPool,
    store: &'a BlobStore,
    config: &'a YoutubeConfig,
    downloader: &'a D,
}

/// Production [`MediaDownloader`] confining one configured yt-dlp binary.
#[derive(Debug, Clone)]
pub struct YtDlpDownloader {
    binary_path: String,
    timeout: Duration,
    max_height: u64,
}

impl YtDlpDownloader {
    /// Builds a runner from explicit confinement parameters.
    #[must_use]
    pub fn from_parts(binary_path: &str, timeout_secs: u64, max_height: u32) -> Self {
        Self {
            binary_path: binary_path.to_owned(),
            timeout: Duration::from_secs(timeout_secs),
            max_height: u64::from(max_height),
        }
    }
}

impl MediaDownloader for YtDlpDownloader {
    async fn download(
        &self,
        video_url: &str,
        max_item_bytes: u64,
        working_dir: &Path,
    ) -> Result<DownloadedMedia, MediaDownloadError> {
        const STDERR_CAP_BYTES: usize = 64 * 1024;
        let output = working_dir.join("media.mp4");
        // Fixed argv template; the URL is a positional argument so it can never be parsed as an
        // option, and the environment is empty because the confined binary needs nothing.
        let format = format!(
            "bv*[height<={height}]+ba/b[height<={height}]",
            height = self.max_height
        );
        let mut command = tokio::process::Command::new(&self.binary_path);
        command
            .args(["--no-playlist", "--no-part", "-f", format.as_str(), "-o"])
            .arg(&output)
            .arg(video_url)
            .current_dir(working_dir)
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| MediaDownloadError {
            class: "media_download_io",
        })?;
        let mut stderr = child.stderr.take();
        let stderr_guard = tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let Some(stderr) = stderr.as_mut() else {
                return Ok::<(), ()>(());
            };
            let mut buffer = [0_u8; 2048];
            let mut total = 0_usize;
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) => return Ok(()),
                    Ok(read) if total + read > STDERR_CAP_BYTES => return Err(()),
                    Ok(read) => total += read,
                    Err(_) => return Err(()),
                }
            }
        });
        let waited = tokio::time::timeout(self.timeout, child.wait()).await;
        let status = match waited {
            Ok(Ok(status)) => status,
            Ok(Err(_)) => {
                return Err(MediaDownloadError {
                    class: "media_download_io",
                });
            }
            Err(_elapsed) => {
                let _ = child.kill().await;
                return Err(MediaDownloadError {
                    class: "media_download_timeout",
                });
            }
        };
        if !matches!(stderr_guard.await, Ok(Ok(()))) {
            return Err(MediaDownloadError {
                class: "media_download_output",
            });
        }
        if !status.success() {
            return Err(MediaDownloadError {
                class: "media_download_exit",
            });
        }
        let length = tokio::fs::metadata(&output)
            .await
            .map_err(|_| MediaDownloadError {
                class: "media_download_io",
            })?
            .len();
        if length > max_item_bytes {
            let _ = tokio::fs::remove_file(&output).await;
            return Err(MediaDownloadError {
                class: "media_over_item_cap",
            });
        }
        Ok(DownloadedMedia {
            path: output,
            length_bytes: length,
        })
    }
}

impl<'a, D: MediaDownloader> MediaArchiver<'a, D> {
    /// Creates an archiver over shared handles and configuration.
    pub fn new(
        pool: &'a sqlx::PgPool,
        store: &'a BlobStore,
        config: &'a YoutubeConfig,
        downloader: &'a D,
    ) -> Self {
        Self {
            pool,
            store,
            config,
            downloader,
        }
    }

    /// Runs one archival attempt for a succeeded run.
    ///
    /// # Errors
    ///
    /// Returns only infrastructure errors from persistence or storage; archival failures are
    /// recorded in [`ArchivalOutcome`] instead.
    pub async fn archive_video(
        &self,
        run_id: uuid::Uuid,
        video_id: &str,
        canonical_url: &str,
    ) -> std::io::Result<ArchivalOutcome> {
        let media = &self.config.media;
        if !media.enabled {
            return Ok(ArchivalOutcome::Disabled);
        }
        if extractor_persistence::has_unexpired_media_for_video(self.pool, video_id)
            .await
            .map_err(infrastructure)?
        {
            return Ok(ArchivalOutcome::SkippedDuplicate);
        }
        let used = unexpired_media_bytes(self.pool)
            .await
            .map_err(infrastructure)?;
        let budget = i64::try_from(media.total_budget_bytes).unwrap_or(i64::MAX);
        if used >= budget {
            return Ok(ArchivalOutcome::SkippedBudgetExhausted);
        }

        let working_dir =
            std::env::temp_dir().join(format!("ratatoskr-youtube-media-{}", uuid::Uuid::now_v7()));
        tokio::fs::create_dir_all(&working_dir).await?;
        let downloaded = match self
            .downloader
            .download(canonical_url, media.max_item_bytes, &working_dir)
            .await
        {
            Ok(downloaded) => downloaded,
            Err(error) => {
                remove_dir(&working_dir).await;
                return Ok(ArchivalOutcome::FailedDownload { class: error.class });
            }
        };
        if downloaded.length_bytes > media.max_item_bytes {
            remove_dir(&working_dir).await;
            return Ok(ArchivalOutcome::FailedDownload {
                class: "media_over_item_cap",
            });
        }

        // The copy cap is the smaller of the item cap and the remaining budget, so neither can
        // be exceeded even when the downloader ignored its own contract.
        let remaining = u64::try_from(budget.saturating_sub(used)).unwrap_or(media.max_item_bytes);
        let copy_cap = media.max_item_bytes.min(remaining);
        let file = match tokio::fs::File::open(&downloaded.path).await {
            Ok(file) => file,
            Err(error) => {
                remove_dir(&working_dir).await;
                return Err(error);
            }
        };
        let stored = self
            .store
            .store("video/mp4", capped_file_stream(file, copy_cap))
            .await;
        match stored {
            Ok(reference) => {
                let record = extractor_persistence::MediaArchiveRecord {
                    run_id,
                    video_id,
                    reference: &reference,
                    retention_hours: retention_hours(media.retention_hours),
                };
                match reserve_media_archive(self.pool, &record, budget).await {
                    Ok(true) => {
                        remove_dir(&working_dir).await;
                        Ok(ArchivalOutcome::Stored {
                            digest_hex: reference.digest.hex.as_str().to_owned(),
                            length_bytes: reference.length_bytes,
                        })
                    }
                    Ok(false) => {
                        remove_dir(&working_dir).await;
                        // A concurrent reservation consumed the budget; the content-addressed
                        // bytes stay as an orphan until a later sweep collects them.
                        Ok(ArchivalOutcome::SkippedBudgetRace)
                    }
                    Err(error) => {
                        remove_dir(&working_dir).await;
                        Err(infrastructure(error))
                    }
                }
            }
            Err(extractor_blob_store::BlobStoreError::Stream) => {
                remove_dir(&working_dir).await;
                Ok(ArchivalOutcome::FailedDownload {
                    class: "media_over_item_cap",
                })
            }
            Err(error) => {
                remove_dir(&working_dir).await;
                Err(infrastructure(error))
            }
        }
    }
}

fn infrastructure(error: impl std::error::Error + Send + Sync + 'static) -> std::io::Error {
    std::io::Error::other(error)
}

async fn remove_dir(path: &Path) {
    let _ = tokio::fs::remove_dir_all(path).await;
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "values above i32::MAX saturate through the explicit branch"
)]
fn retention_hours(configured: u64) -> i32 {
    if configured > i32::MAX as u64 {
        i32::MAX
    } else {
        configured as i32
    }
}

/// Size of one streaming chunk copied into content-addressed storage.
const CHUNK_BYTES: usize = 64 * 1024;

/// Streams one file into blob storage while aborting the moment `cap` would be exceeded.
///
/// An exact-fit file completes normally; any byte beyond the cap turns into a stream error so
/// blob storage discards its staging copy whole.
fn capped_file_stream(
    file: tokio::fs::File,
    cap: u64,
) -> std::pin::Pin<Box<impl futures_util::Stream<Item = Result<bytes::Bytes, std::io::Error>>>> {
    Box::pin(futures_util::stream::unfold(
        (file, cap, 0_u64, false, false),
        |(mut file, cap, total, eof, exceeded)| async move {
            use tokio::io::AsyncReadExt as _;
            if exceeded || eof {
                return None;
            }
            let mut buffer = vec![0_u8; CHUNK_BYTES];
            match file.read(&mut buffer).await {
                Ok(0) => None,
                Ok(read) => {
                    if total >= cap {
                        return Some((
                            Err(std::io::Error::other("item cap exceeded")),
                            (file, cap, total, eof, true),
                        ));
                    }
                    let remaining = cap - total;
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "CHUNK_BYTES bounds `read` far below usize::MAX"
                    )]
                    let take = (read as u64).min(remaining);
                    #[allow(
                        clippy::cast_possible_truncation,
                        reason = "`take` never exceeds this chunk's `read` length"
                    )]
                    let take_usize = take as usize;
                    if take_usize < read {
                        // More bytes exist beyond the cap; abort instead of storing partials.
                        return Some((
                            Err(std::io::Error::other("item cap exceeded")),
                            (file, cap, total, eof, true),
                        ));
                    }
                    buffer.truncate(take_usize);
                    let eof = read < CHUNK_BYTES;
                    Some((
                        Ok(bytes::Bytes::from(buffer)),
                        (file, cap, total + take, eof, false),
                    ))
                }
                Err(error) => Some((Err(error), (file, cap, total, eof, true))),
            }
        },
    ))
}
