//! Telegram-owned PDF blobs through the extractor pipeline (XR-021 CONTRACTS.md S11).

use async_nats::jetstream;
use extractor_blob_store::BlobStore;
use extractor_core::ExtractorConfig;
use extractor_eventing::{QueuedRun, RenderBus, claim_queued_run, consume_capture};
use extractor_persistence::test_support::TestDatabase;
use extractor_safe_fetch::SafeFetcher;
use extractor_test_support::TemporaryBlobRoot;
use extractor_test_support::capture::CaptureCommandJson;
use futures_util::stream;
use ratatoskr_document_contracts::ContentDocumentExtracted;
use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_identifiers::BlobRef;

const SUBJECT: &str = "cmd.content.capture.requested.v1";
const TEXT_PDF: &[u8] = include_bytes!("../../../crates/pdf/tests/fixtures/text-two-pages.pdf");
const NO_TEXT_PDF: &[u8] = include_bytes!("../../../crates/pdf/tests/fixtures/no-text-layer.pdf");

/// One database, an extractor store, and the Telegram service's blob root, all temporary.
struct Fixture {
    database: TestDatabase,
    extractor_root: TemporaryBlobRoot,
    telegram_root: TemporaryBlobRoot,
}

impl Fixture {
    async fn create() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            database: TestDatabase::create().await?,
            extractor_root: TemporaryBlobRoot::create().await?,
            telegram_root: TemporaryBlobRoot::create().await?,
        })
    }

    fn extractor_store(&self) -> BlobStore {
        BlobStore::new(self.extractor_root.path())
    }

    fn telegram_store(&self) -> Result<BlobStore, Box<dyn std::error::Error>> {
        Ok(BlobStore::new(self.telegram_root.path()).with_owner("ratatoskr-telegram")?)
    }

    /// Stores `bytes` the way the Telegram service does and returns the reference it would send.
    async fn telegram_blob(
        &self,
        media_type: &str,
        bytes: &'static [u8],
    ) -> Result<BlobRef, Box<dyn std::error::Error>> {
        Ok(self
            .telegram_store()?
            .store(
                media_type,
                stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from_static(bytes))]),
            )
            .await?)
    }

    /// Queues and leases one blob run for `blob`.
    async fn lease(&self, blob: &BlobRef) -> Result<QueuedRun, Box<dyn std::error::Error>> {
        let command = CaptureCommandJson::blob(
            blob.owner_service.as_str(),
            blob.digest.hex.as_str(),
            blob.media_type.as_str(),
            blob.length_bytes,
        );
        let pool = self.database.database.pool();
        consume_capture(pool, SUBJECT, &command.to_bytes()).await?;
        Ok(claim_queued_run(pool, "test-worker", 60)
            .await?
            .ok_or("the blob command did not produce queued work")?)
    }

    async fn process(&self, run: &QueuedRun) -> Result<(), Box<dyn std::error::Error>> {
        let config = ExtractorConfig::built_in(self.extractor_root.path());
        let store = self.extractor_store();
        let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
        let bus = RenderBus::new(
            jetstream::new(async_nats::connect(&nats_url()).await?),
            true,
        );
        Box::pin(extractor_service::process_run(
            self.database.database.pool(),
            &fetcher,
            &store,
            &self.telegram_store()?,
            &config.parser,
            &config.pdf,
            &config.providers,
            &config.render,
            &config.youtube,
            &bus,
            run,
        ))
        .await?;
        Ok(())
    }

    async fn outcome(
        &self,
        run: &QueuedRun,
    ) -> Result<(String, Option<String>), Box<dyn std::error::Error>> {
        Ok(sqlx::query_as(
            "select status, last_error_class from extractor.extraction_runs where run_id = $1",
        )
        .bind(run.run_id)
        .fetch_one(self.database.database.pool())
        .await?)
    }
}

#[tokio::test]
async fn blob_pdf_run_completes_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::create().await?;
    let pool = fixture.database.database.pool();
    let peer = fixture.telegram_blob("application/pdf", TEXT_PDF).await?;
    let run = fixture.lease(&peer).await?;

    Box::pin(fixture.process(&run)).await?;

    assert_eq!(fixture.outcome(&run).await?, ("succeeded".to_owned(), None));
    let urn = format!("urn:ratatoskr:blob:sha256:{}", peer.digest.hex.as_str());
    let payload: serde_json::Value = sqlx::query_scalar(
        "select payload from extractor.outbox_events
          where subject = 'evt.content.document.extracted.v1'",
    )
    .fetch_one(pool)
    .await?;
    let fact = serde_json::from_value::<EventEnvelope>(payload)?
        .payload_as::<ContentDocumentExtracted>()?;
    fact.validate()?;
    assert_eq!(fact.document.source_address.as_str(), urn);
    let provenance = fact
        .document
        .provenance
        .first()
        .ok_or("the document must carry provenance")?;
    assert_eq!(
        provenance.source_blob.owner_service.as_str(),
        "ratatoskr-extractor"
    );
    assert_eq!(provenance.source_blob.digest, peer.digest);
    assert_eq!(
        provenance.source_blob.media_type.as_str(),
        "application/pdf"
    );

    let artifacts: Vec<(String, String)> = sqlx::query_as(
        "select kind, digest_hex from extractor.artifacts where run_id = $1 order by kind",
    )
    .bind(run.run_id)
    .fetch_all(pool)
    .await?;
    assert_eq!(
        artifacts,
        vec![
            (
                "document_ir".to_owned(),
                fact.document_blob.digest.hex.as_str().to_owned()
            ),
            ("raw_source".to_owned(), peer.digest.hex.as_str().to_owned()),
        ]
    );
    let fetches: i64 =
        sqlx::query_scalar("select count(*) from extractor.fetches where run_id = $1")
            .bind(run.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(fetches, 0, "a blob run performs no network fetch");
    let reports: i64 = sqlx::query_scalar(
        "select count(*) from extractor.outbox_events
          where subject = 'evt.platform.operation.reported.v1'
            and payload->'payload'->>'status' = 'succeeded'",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(reports, 1);

    fixture
        .extractor_store()
        .verify(&provenance.source_blob)
        .await?;
    fixture.database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn blob_run_with_missing_peer_file_fails_blob_missing()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::create().await?;
    let peer = fixture.telegram_blob("application/pdf", TEXT_PDF).await?;
    let run = fixture.lease(&peer).await?;
    tokio::fs::remove_file(fixture.telegram_store()?.resolve(&peer)?).await?;

    Box::pin(fixture.process(&run)).await?;

    assert_eq!(
        fixture.outcome(&run).await?,
        ("failed".to_owned(), Some("blob_missing".to_owned()))
    );
    fixture.database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn blob_run_with_tampered_peer_bytes_fails_blob_mismatch()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::create().await?;
    let peer = fixture.telegram_blob("application/pdf", TEXT_PDF).await?;
    let run = fixture.lease(&peer).await?;
    let path = fixture.telegram_store()?.resolve(&peer)?;
    let mut tampered = tokio::fs::read(&path).await?;
    if let Some(byte) = tampered.first_mut() {
        *byte ^= 0xff;
    }
    tokio::fs::write(&path, tampered).await?;

    Box::pin(fixture.process(&run)).await?;

    assert_eq!(
        fixture.outcome(&run).await?,
        ("failed".to_owned(), Some("blob_mismatch".to_owned()))
    );
    fixture.database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn blob_run_with_foreign_owner_fails_blob_owner() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::create().await?;
    let peer = fixture.telegram_blob("application/pdf", TEXT_PDF).await?;
    let foreign = BlobRef {
        owner_service: ratatoskr_identifiers::BlobOwner::parse("ratatoskr-knowledge")?,
        ..peer
    };
    let run = fixture.lease(&foreign).await?;

    Box::pin(fixture.process(&run)).await?;

    assert_eq!(
        fixture.outcome(&run).await?,
        ("failed".to_owned(), Some("blob_owner".to_owned()))
    );
    fixture.database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn blob_run_with_image_media_type_fails_unsupported_media()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::create().await?;
    let peer = fixture.telegram_blob("image/png", TEXT_PDF).await?;
    let run = fixture.lease(&peer).await?;

    Box::pin(fixture.process(&run)).await?;

    assert_eq!(
        fixture.outcome(&run).await?,
        ("failed".to_owned(), Some("unsupported_media".to_owned()))
    );
    fixture.database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn blob_pdf_without_text_layer_records_pdf_no_text_layer()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::create().await?;
    let pool = fixture.database.database.pool();
    let peer = fixture
        .telegram_blob("application/pdf", NO_TEXT_PDF)
        .await?;
    let run = fixture.lease(&peer).await?;

    Box::pin(fixture.process(&run)).await?;

    assert_eq!(
        fixture.outcome(&run).await?,
        ("failed".to_owned(), Some("pdf_no_text_layer".to_owned()))
    );
    let facts: (i64, i64, i64) = sqlx::query_as(
        "select
            (select count(*) from extractor.artifacts where run_id = $1 and kind = 'raw_source'),
            (select count(*) from extractor.fetches where run_id = $1),
            (select count(*) from extractor.outbox_events
              where subject = 'evt.content.document.extracted.v1')",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        facts,
        (1, 0, 0),
        "the raw source is kept as evidence, no fetch row exists, and no document fact is published"
    );
    fixture.database.cleanup().await?;
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only broker location is not process configuration"
)]
fn nats_url() -> String {
    match std::env::var("EXTRACTOR_TEST_NATS_URL") {
        Ok(value) => value,
        Err(_) => "nats://127.0.0.1:4222".to_owned(),
    }
}
