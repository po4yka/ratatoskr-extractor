//! Gated media archival tests: caps, budgets, duplicates, and the confined runner.
//!
//! All downloader behaviour is faked or served by local fixture scripts; no network access and
//! no real yt-dlp binary participate in this suite.

use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use extractor_blob_store::BlobStore;
use extractor_core::ExtractorConfig;
use extractor_persistence::reserve_media_archive;
use extractor_persistence::test_support::TestDatabase;
use extractor_persistence::unexpired_media_bytes;
use extractor_service::ArchivalOutcome;
use extractor_service::DownloadedMedia;
use extractor_service::MediaArchiver;
use extractor_service::MediaDownloadError;
use extractor_service::MediaDownloader;
use extractor_service::YtDlpDownloader;
use extractor_test_support::TemporaryBlobRoot;

const VIDEO_A: &str = "AAAAAAAAAAA";
const VIDEO_B: &str = "BBBBBBBBBBB";

/// Counts invocations and always succeeds with one tiny file.
struct TinyFake {
    calls: AtomicUsize,
}

impl MediaDownloader for TinyFake {
    async fn download(
        &self,
        _video_url: &str,
        _max_item_bytes: u64,
        working_dir: &Path,
    ) -> Result<DownloadedMedia, MediaDownloadError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let path = working_dir.join("tiny.bin");
        tokio::fs::write(&path, b"ok")
            .await
            .map_err(|_| MediaDownloadError {
                class: "media_download_io",
            })?;
        Ok(DownloadedMedia {
            path,
            length_bytes: 2,
        })
    }
}

/// Ignores its cap argument and produces an oversized file, violating the contract.
struct OversizedFake {
    size: u64,
}

impl MediaDownloader for OversizedFake {
    async fn download(
        &self,
        _video_url: &str,
        _max_item_bytes: u64,
        working_dir: &Path,
    ) -> Result<DownloadedMedia, MediaDownloadError> {
        use tokio::io::AsyncWriteExt as _;
        let path = working_dir.join("big.bin");
        let chunk = vec![b'x'; 4096];
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|_| MediaDownloadError {
                class: "media_download_io",
            })?;
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the chunk is 4096 bytes, far below usize::MAX"
        )]
        let chunk_len = chunk.len() as u64;
        let mut remaining = self.size;
        while remaining >= chunk_len {
            file.write_all(&chunk)
                .await
                .map_err(|_| MediaDownloadError {
                    class: "media_download_io",
                })?;
            remaining -= chunk_len;
        }
        #[allow(clippy::cast_possible_truncation, reason = "remainder < chunk length")]
        let tail_length = remaining as usize;
        if tail_length > 0 {
            file.write_all(chunk.get(..tail_length).unwrap_or(&chunk))
                .await
                .map_err(|_| MediaDownloadError {
                    class: "media_download_io",
                })?;
        }
        Ok(DownloadedMedia {
            path,
            length_bytes: self.size,
        })
    }
}

/// Always fails with a stable class.
struct FailingFake;

impl MediaDownloader for FailingFake {
    async fn download(
        &self,
        _video_url: &str,
        _max_item_bytes: u64,
        _working_dir: &Path,
    ) -> Result<DownloadedMedia, MediaDownloadError> {
        Err(MediaDownloadError {
            class: "media_download_timeout",
        })
    }
}

fn enabled_config(budget: u64, item_cap: u64) -> ExtractorConfig {
    let root = std::env::temp_dir();
    let mut config = ExtractorConfig::built_in(&root);
    config.youtube.media.enabled = true;
    config.youtube.media.total_budget_bytes = budget;
    config.youtube.media.max_item_bytes = item_cap;
    config
}

/// Queues one run and marks it succeeded so accounting accepts its rows.
async fn succeeded_run(
    pool: &sqlx::PgPool,
    url: &str,
) -> Result<uuid::Uuid, Box<dyn std::error::Error>> {
    let command_id = uuid::Uuid::now_v7();
    let operation_id = uuid::Uuid::now_v7();
    let owner_id = uuid::Uuid::now_v7();
    let source_id = uuid::Uuid::now_v7();
    let run_id = uuid::Uuid::now_v7();
    let document_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into extractor.inbox_events (command_id, subject, command_type, producer, received_at)
         values ($1, 'cmd.content.capture.requested.v1', 'content.capture.requested.v1',
                 'ratatoskr-platform', transaction_timestamp())",
    )
    .bind(command_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.sources
             (source_id, owner_id, original_url, normalized_url, canonical_url, host,
              classification, created_at)
         values ($1, $2, $3, $3, $3, 'www.youtube.com', 'youtube', transaction_timestamp())",
    )
    .bind(source_id)
    .bind(owner_id)
    .bind(url)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.extraction_runs
             (run_id, command_id, operation_id, owner_id, correlation_id, source_id, document_id,
              status, policy_version, normalizer_version, parser_version, queued_at, started_at,
              completed_at)
         values ($1, $2, $3, $4, $5, $6, $7, 'succeeded', 'ssrf-v1', 'url-v1', 'youtube-v1',
                 transaction_timestamp(), transaction_timestamp(), transaction_timestamp())",
    )
    .bind(run_id)
    .bind(command_id)
    .bind(operation_id)
    .bind(owner_id)
    .bind(format!("operation:{operation_id}"))
    .bind(source_id)
    .bind(document_id)
    .execute(pool)
    .await?;
    Ok(run_id)
}

/// Stores a real blob and reserves accounting for one video under one run.
async fn seed_archive(
    pool: &sqlx::PgPool,
    store: &BlobStore,
    run_id: uuid::Uuid,
    video_id: &str,
    payload: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let chunk = bytes::Bytes::copy_from_slice(payload);
    let reference = store
        .store(
            "video/mp4",
            futures_util::stream::iter([Ok::<_, std::io::Error>(chunk)]),
        )
        .await?;
    let record = extractor_persistence::MediaArchiveRecord {
        run_id,
        video_id,
        reference: &reference,
        retention_hours: 24,
    };
    assert!(reserve_media_archive(pool, &record, i64::MAX).await?);
    Ok(())
}

#[tokio::test]
async fn disabled_gate_never_invokes_the_downloader() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let config = ExtractorConfig::built_in(root.path());
    assert!(!config.youtube.media.enabled, "the gate defaults off");
    let fake = TinyFake {
        calls: AtomicUsize::new(0),
    };
    let archiver = MediaArchiver::new(pool, &store, &config.youtube, &fake);
    let outcome = archiver
        .archive_video(
            uuid::Uuid::nil(),
            VIDEO_A,
            "https://www.youtube.com/watch?v=A",
        )
        .await?;
    assert_eq!(outcome, ArchivalOutcome::Disabled);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn oversized_item_aborts_and_writes_no_accounting_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let run_id = succeeded_run(pool, "https://www.youtube.com/watch?v=A").await?;
    let config = enabled_config(1 << 30, 1024);
    let fake = OversizedFake { size: 4096 };
    let archiver = MediaArchiver::new(pool, &store, &config.youtube, &fake);
    let outcome = archiver
        .archive_video(run_id, VIDEO_A, "https://www.youtube.com/watch?v=A")
        .await?;
    assert_eq!(
        outcome,
        ArchivalOutcome::FailedDownload {
            class: "media_over_item_cap"
        }
    );
    assert_eq!(unexpired_media_bytes(pool).await?, 0);
    let (rows,): (i64,) =
        sqlx::query_as("select count(*) from extractor.artifacts where kind = 'archived_media'")
            .fetch_one(pool)
            .await?;
    assert_eq!(rows, 0);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn exhausted_total_budget_skips_before_any_download() -> Result<(), Box<dyn std::error::Error>>
{
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let seeded_run = succeeded_run(pool, "https://www.youtube.com/watch?v=S").await?;
    seed_archive(pool, &store, seeded_run, VIDEO_A, &[0u8; 512]).await?;
    let config = enabled_config(512, 1 << 30);
    let fake = TinyFake {
        calls: AtomicUsize::new(0),
    };
    let other_run = succeeded_run(pool, "https://www.youtube.com/watch?v=B").await?;
    let archiver = MediaArchiver::new(pool, &store, &config.youtube, &fake);
    let outcome = archiver
        .archive_video(other_run, VIDEO_B, "https://www.youtube.com/watch?v=B")
        .await?;
    assert_eq!(outcome, ArchivalOutcome::SkippedBudgetExhausted);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn duplicate_video_skips_while_unexpired() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let seeded_run = succeeded_run(pool, "https://www.youtube.com/watch?v=A").await?;
    seed_archive(pool, &store, seeded_run, VIDEO_A, &[0u8; 128]).await?;
    let config = enabled_config(1 << 30, 1 << 30);
    let fake = TinyFake {
        calls: AtomicUsize::new(0),
    };
    let replay_run = succeeded_run(pool, "https://www.youtube.com/watch?v=A").await?;
    let archiver = MediaArchiver::new(pool, &store, &config.youtube, &fake);
    let outcome = archiver
        .archive_video(replay_run, VIDEO_A, "https://www.youtube.com/watch?v=A")
        .await?;
    assert_eq!(outcome, ArchivalOutcome::SkippedDuplicate);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn downloader_failure_records_its_class_and_no_rows() -> Result<(), Box<dyn std::error::Error>>
{
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let run_id = succeeded_run(pool, "https://www.youtube.com/watch?v=A").await?;
    let config = enabled_config(1 << 30, 1 << 30);
    let archiver = MediaArchiver::new(pool, &store, &config.youtube, &FailingFake);
    let outcome = archiver
        .archive_video(run_id, VIDEO_A, "https://www.youtube.com/watch?v=A")
        .await?;
    assert_eq!(
        outcome,
        ArchivalOutcome::FailedDownload {
            class: "media_download_timeout"
        }
    );
    assert_eq!(unexpired_media_bytes(pool).await?, 0);
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn stored_media_commits_accounting_within_remaining_budget()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let run_id = succeeded_run(pool, "https://www.youtube.com/watch?v=A").await?;
    let config = enabled_config(4096, 4096);
    let fake = TinyFake {
        calls: AtomicUsize::new(0),
    };
    let archiver = MediaArchiver::new(pool, &store, &config.youtube, &fake);
    let outcome = archiver
        .archive_video(run_id, VIDEO_A, "https://www.youtube.com/watch?v=A")
        .await?;
    let ArchivalOutcome::Stored {
        digest_hex,
        length_bytes,
    } = outcome
    else {
        return Err(format!("expected Stored, got {outcome:?}").into());
    };
    assert_eq!(length_bytes, 2);
    assert_eq!(digest_hex.len(), 64);
    assert_eq!(unexpired_media_bytes(pool).await?, 2);
    database.cleanup().await?;
    Ok(())
}

// --- Confined runner against local fixture scripts -------------------------------------------

/// Writes a fixture shell script and returns its path.
async fn fixture_script(body: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("yt-fake-{}.sh", uuid::Uuid::now_v7()));
    tokio::fs::write(&path, format!("#!/bin/sh\n{body}")).await?;
    let mut permissions = tokio::fs::metadata(&path).await?.permissions();
    #[allow(
        clippy::permissions_set_readonly_false,
        reason = "granting execute is the point"
    )]
    {
        permissions.set_mode(0o755);
    }
    tokio::fs::set_permissions(&path, permissions).await?;
    Ok(path)
}

fn tempfile_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("yt-runner-{}", uuid::Uuid::now_v7()))
}

#[tokio::test]
async fn runner_builds_the_confined_argv_contract() -> Result<(), Box<dyn std::error::Error>> {
    // Arg positions follow the documented template: 1 --no-playlist, 2 --no-part, 3 -f,
    // 4 format, 5 -o, 6 output path, 7 url. The script records argv and materializes media.
    let script =
        fixture_script("printf '%s\\n' \"$@\" > \"$PWD/args.txt\"\nprintf 'MEDIABYTES' > \"$6\"\n")
            .await?;
    let working = tempfile_dir();
    tokio::fs::create_dir_all(&working).await?;
    let runner = YtDlpDownloader::from_parts(script.to_string_lossy().as_ref(), 60, 1080);
    let media = runner
        .download(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            4096,
            &working,
        )
        .await?;
    assert_eq!(media.length_bytes, 10);
    let args = tokio::fs::read_to_string(working.join("args.txt")).await?;
    let lines: Vec<&str> = args.lines().collect();
    assert_eq!(lines.first(), Some(&"--no-playlist"));
    assert!(lines.contains(&"-f"));
    assert!(
        lines.iter().any(|l| l.contains("height<=1080")),
        "format must cap height: {args}"
    );
    assert_eq!(
        lines.last(),
        Some(&"https://www.youtube.com/watch?v=dQw4w9WgXcQ")
    );
    let recorded_output = lines
        .iter()
        .enumerate()
        .find(|(index, line)| **line == "-o" && *index + 1 < lines.len())
        .map(|(index, _)| lines[index + 1]);
    let output_path = recorded_output.expect("argv must carry -o");
    assert!(
        Path::new(output_path).starts_with(&working),
        "output stays in working dir"
    );
    let _ = tokio::fs::remove_dir_all(&working).await;
    let _ = tokio::fs::remove_dir_all(script.parent().expect("script has parent")).await;
    Ok(())
}

#[tokio::test]
async fn runner_kills_a_sleeping_binary_at_the_deadline() -> Result<(), Box<dyn std::error::Error>>
{
    let script = fixture_script("sleep 30\n").await?;
    let working = tempfile_dir();
    tokio::fs::create_dir_all(&working).await?;
    let runner = YtDlpDownloader::from_parts(script.to_string_lossy().as_ref(), 1, 1080);
    let started = std::time::Instant::now();
    let error = runner
        .download(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            4096,
            &working,
        )
        .await
        .expect_err("sleeping binary must time out");
    assert_eq!(error.class, "media_download_timeout");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "kill must be prompt"
    );
    let _ = tokio::fs::remove_dir_all(&working).await;
    let _ = tokio::fs::remove_dir_all(script.parent().expect("script has parent")).await;
    Ok(())
}

#[tokio::test]
async fn runner_maps_nonzero_exit_to_its_class() -> Result<(), Box<dyn std::error::Error>> {
    let script = fixture_script("exit 3\n").await?;
    let working = tempfile_dir();
    tokio::fs::create_dir_all(&working).await?;
    let runner = YtDlpDownloader::from_parts(script.to_string_lossy().as_ref(), 60, 1080);
    let error = runner
        .download(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            4096,
            &working,
        )
        .await
        .expect_err("nonzero exit must fail");
    assert_eq!(error.class, "media_download_exit");
    let _ = tokio::fs::remove_dir_all(&working).await;
    let _ = tokio::fs::remove_dir_all(script.parent().expect("script has parent")).await;
    Ok(())
}

#[tokio::test]
async fn runner_caps_captured_stderr() -> Result<(), Box<dyn std::error::Error>> {
    let script = fixture_script("head -c 262144 /dev/zero | tr '\\0' 'x' >&2\nexit 1\n").await?;
    let working = tempfile_dir();
    tokio::fs::create_dir_all(&working).await?;
    let runner = YtDlpDownloader::from_parts(script.to_string_lossy().as_ref(), 60, 1080);
    let error = runner
        .download(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            4096,
            &working,
        )
        .await
        .expect_err("stderr flood must be bounded and mapped");
    assert_eq!(error.class, "media_download_output");
    let _ = tokio::fs::remove_dir_all(&working).await;
    let _ = tokio::fs::remove_dir_all(script.parent().expect("script has parent")).await;
    Ok(())
}
