//! One document identity per owner and content digest, whatever route delivered the bytes
//! (XR-021 CONTRACTS.md R5-02 rule 5).

use async_nats::jetstream;
use extractor_blob_store::BlobStore;
use extractor_core::ExtractorConfig;
use extractor_eventing::{QueuedRun, RenderBus, claim_queued_run, consume_capture};
use extractor_persistence::test_support::TestDatabase;
use extractor_safe_fetch::SafeFetcher;
use extractor_test_support::capture::CaptureCommandJson;
use extractor_test_support::{
    MAX_EVENT_BYTES, ScriptedResponse, ScriptedServer, TemporaryBlobRoot,
};
use futures_util::stream;
use ratatoskr_document_contracts::{ContentDocumentExtracted, Document};
use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_identifiers::BlobRef;

const SUBJECT: &str = "cmd.content.capture.requested.v1";
const TEXT_PDF: &[u8] = include_bytes!("../../../crates/pdf/tests/fixtures/text-two-pages.pdf");
const OTHER_TEXT_PDF: &[u8] = include_bytes!("../../../crates/pdf/tests/fixtures/multi-column.pdf");

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
        bytes: &'static [u8],
    ) -> Result<BlobRef, Box<dyn std::error::Error>> {
        Ok(self
            .telegram_store()?
            .store(
                "application/pdf",
                stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from_static(bytes))]),
            )
            .await?)
    }

    /// Captures `bytes` as a Telegram blob on behalf of `owner` and extracts it.
    async fn capture_blob(
        &self,
        owner: uuid::Uuid,
        bytes: &'static [u8],
    ) -> Result<Document, Box<dyn std::error::Error>> {
        let blob = self.telegram_blob(bytes).await?;
        let mut command = CaptureCommandJson::blob(
            blob.owner_service.as_str(),
            blob.digest.hex.as_str(),
            blob.media_type.as_str(),
            blob.length_bytes,
        )
        .with_idempotency(&format!("blob-{}", uuid::Uuid::now_v7()));
        command.tenant_user = owner;
        let config = ExtractorConfig::built_in(self.extractor_root.path());
        Box::pin(self.run(&command, &config)).await
    }

    /// Captures `bytes` as a URL on behalf of `owner`, served by a local scripted server.
    async fn capture_url(
        &self,
        owner: uuid::Uuid,
        bytes: &'static [u8],
    ) -> Result<Document, Box<dyn std::error::Error>> {
        let server = ScriptedServer::start(vec![
            ScriptedResponse::chunks([bytes::Bytes::from_static(bytes)]).with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/pdf"),
            ),
        ])
        .await?;
        // A hostname keeps the public literal-address policy applicable exactly as in production.
        let url = server.uri("/report.pdf").replace("127.0.0.1", "localhost");
        let mut config = ExtractorConfig::built_in(self.extractor_root.path());
        config.fetch.allowed_ports = vec![server.port()];
        config.fetch.total_timeout_ms = 10_000;
        // Intake refuses a non-standard port, so the run is queued the way intake would for a
        // standard one: the same rows, with the same owner, and the same pipeline from there on.
        let operation_id = self.queue_url_run(owner, &url).await?;
        let document = Box::pin(self.process_next(operation_id, &config)).await?;
        assert_eq!(server.request_count(), 1, "the URL is fetched exactly once");
        Ok(document)
    }

    /// Mirrors the production intake rows for one URL run owned by `owner`.
    async fn queue_url_run(
        &self,
        owner: uuid::Uuid,
        url: &str,
    ) -> Result<uuid::Uuid, Box<dyn std::error::Error>> {
        let pool = self.database.database.pool();
        let command_id = uuid::Uuid::now_v7();
        let operation_id = uuid::Uuid::now_v7();
        let source_id = uuid::Uuid::now_v7();
        sqlx::query(
            "insert into extractor.inbox_events
                 (command_id, subject, command_type, producer, received_at)
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
             values ($1, $2, $3, $3, $3, 'localhost', 'pdf', transaction_timestamp())",
        )
        .bind(source_id)
        .bind(owner)
        .bind(url)
        .execute(pool)
        .await?;
        sqlx::query(
            "insert into extractor.extraction_runs
                 (run_id, command_id, operation_id, owner_id, correlation_id, source_id,
                  status, policy_version, normalizer_version, parser_version, queued_at)
             values ($1, $2, $3, $4, $5, $6, 'queued', 'ssrf-v1', 'url-v1', 'html-v1',
                     transaction_timestamp())",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(command_id)
        .bind(operation_id)
        .bind(owner)
        .bind(format!("operation:{operation_id}"))
        .bind(source_id)
        .execute(pool)
        .await?;
        Ok(operation_id)
    }

    /// Queues one command, then extracts it.
    async fn run(
        &self,
        command: &CaptureCommandJson,
        config: &ExtractorConfig,
    ) -> Result<Document, Box<dyn std::error::Error>> {
        consume_capture(self.database.database.pool(), SUBJECT, &command.to_bytes()).await?;
        Box::pin(self.process_next(command.operation_id, config)).await
    }

    /// Leases and processes the oldest queued run, then returns the document its event carries.
    async fn process_next(
        &self,
        operation_id: uuid::Uuid,
        config: &ExtractorConfig,
    ) -> Result<Document, Box<dyn std::error::Error>> {
        let pool = self.database.database.pool();
        let run = claim_queued_run(pool, "test-worker", 60, MAX_EVENT_BYTES)
            .await?
            .ok_or("the command did not produce queued work")?;
        self.process(&run, config).await?;
        let status: String =
            sqlx::query_scalar("select status from extractor.extraction_runs where run_id = $1")
                .bind(run.run_id)
                .fetch_one(pool)
                .await?;
        assert_eq!(status, "succeeded");
        let payload: serde_json::Value = sqlx::query_scalar(
            "select payload from extractor.outbox_events
              where subject = 'evt.content.document.extracted.v1' and operation_id = $1",
        )
        .bind(operation_id)
        .fetch_one(pool)
        .await?;
        let fact = serde_json::from_value::<EventEnvelope>(payload)?
            .payload_as::<ContentDocumentExtracted>()?;
        fact.validate()?;
        Ok(fact.document)
    }

    async fn process(
        &self,
        run: &QueuedRun,
        config: &ExtractorConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
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
}

#[tokio::test]
async fn the_same_pdf_by_blob_and_by_url_yields_one_document_id()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::create().await?;
    let owner = uuid::Uuid::now_v7();

    let by_blob = fixture.capture_blob(owner, TEXT_PDF).await?;
    let by_url = fixture.capture_url(owner, TEXT_PDF).await?;

    assert_ne!(
        by_blob.source_address, by_url.source_address,
        "the two routes name different sources"
    );
    assert_eq!(by_blob.content_digest, by_url.content_digest);
    assert_eq!(by_blob.document_id, by_url.document_id);
    fixture.database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_pdf_with_other_text_yields_another_document_id() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::create().await?;
    let owner = uuid::Uuid::now_v7();

    let first = fixture.capture_blob(owner, TEXT_PDF).await?;
    let other = fixture.capture_blob(owner, OTHER_TEXT_PDF).await?;

    assert_ne!(first.content_digest, other.content_digest);
    assert_ne!(first.document_id, other.document_id);
    fixture.database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn the_same_pdf_for_another_owner_yields_another_document_id()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::create().await?;

    let first = fixture.capture_blob(uuid::Uuid::now_v7(), TEXT_PDF).await?;
    let other_owner = fixture.capture_blob(uuid::Uuid::now_v7(), TEXT_PDF).await?;

    assert_eq!(first.content_digest, other_owner.content_digest);
    assert_ne!(first.document_id, other_owner.document_id);
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
