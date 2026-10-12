//! Atomic successful extraction completion.

use extractor_blob_store::BlobStore;
use extractor_document_ir::{CandidateDecision, QualityMetrics, QualityReason, stable_document_id};
use extractor_eventing::{
    CompletedFetch, Completion, claim_queued_run, complete_document, consume_capture, fail_run,
    store_document_ir,
};
use extractor_persistence::test_support::TestDatabase;
use extractor_test_support::capture::CaptureCommandJson;
use extractor_test_support::{MAX_EVENT_BYTES, TemporaryBlobRoot};
use ratatoskr_document_contracts::{
    ContentDocumentExtracted, Document, DocumentAddress, DocumentBlock, DocumentProvenance,
    ExtractionStrategy,
};
use ratatoskr_event_envelope::EventEnvelope;
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, BlockId, ContentDigest, DigestAlgorithm, DigestHex, MediaType, UserId,
};
use ratatoskr_operation_contracts::{OperationReported, OperationStatus};
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;

const SUBJECT: &str = "cmd.content.capture.requested.v1";

#[tokio::test]
async fn completed_document_and_report_commit_with_one_run()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let capture = CaptureCommandJson::url("https://example.test/article")
        .with_idempotency("capture-completed-once");
    let operation_id = capture.operation_id;
    let command = capture.to_bytes();
    consume_capture(database.database.pool(), SUBJECT, &command).await?;
    let run = claim_queued_run(database.database.pool(), "test-worker", 60, MAX_EVENT_BYTES)
        .await?
        .ok_or("the command did not produce queued work")?;
    let run_id = run.run_id;
    let source = blob_ref("text/html", 17, 'a')?;
    let document = document(source.clone(), run.owner)?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let ir_blob = store_document_ir(&store, &document).await?;
    let stored = tokio::fs::read(store.resolve(&ir_blob)?).await?;
    assert_eq!(
        stored,
        ratatoskr_identifiers::canonical_json(&document)?.as_bytes()
    );
    let fetch = CompletedFetch {
        final_url: "https://example.test/article",
        http_status: 200,
        media_type: "text/html",
        wire_bytes: 17,
        decoded_bytes: 17,
        attempts: 1,
        cache_outcome: "fresh",
        etag: None,
        last_modified: None,
        raw_blob: &source,
    };
    let candidates = candidate_decisions();

    assert_eq!(
        complete_document(
            database.database.pool(),
            &run,
            &document,
            &ir_blob,
            &fetch,
            &candidates,
            &[],
        )
        .await?,
        Completion::Applied
    );
    assert_eq!(
        complete_document(
            database.database.pool(),
            &run,
            &document,
            &ir_blob,
            &fetch,
            &candidates,
            &[],
        )
        .await?,
        Completion::Duplicate
    );

    verify_completion(
        database.database.pool(),
        run_id,
        operation_id,
        &document,
        &ir_blob,
    )
    .await?;

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn document_event_payload_is_the_contract_type() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let capture = CaptureCommandJson::url("https://example.test/article")
        .with_idempotency("capture-contract-payload");
    let command_id = capture.command_id;
    let correlation = capture.correlation_id.clone();
    let tenant = capture.tenant_user;
    consume_capture(database.database.pool(), SUBJECT, &capture.to_bytes()).await?;
    let run = claim_queued_run(database.database.pool(), "test-worker", 60, MAX_EVENT_BYTES)
        .await?
        .ok_or("the command did not produce queued work")?;
    let source = blob_ref("text/html", 17, 'a')?;
    let document = document(source.clone(), run.owner)?;
    let document_id = document.document_id.0;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let ir_blob = store_document_ir(&store, &document).await?;
    let fetch = CompletedFetch {
        final_url: "https://example.test/article",
        http_status: 200,
        media_type: "text/html",
        wire_bytes: 17,
        decoded_bytes: 17,
        attempts: 1,
        cache_outcome: "fresh",
        etag: None,
        last_modified: None,
        raw_blob: &source,
    };
    complete_document(
        database.database.pool(),
        &run,
        &document,
        &ir_blob,
        &fetch,
        &candidate_decisions(),
        &[],
    )
    .await?;

    let payload: serde_json::Value = sqlx::query_scalar(
        "select payload from extractor.outbox_events
          where subject = 'evt.content.document.extracted.v1'",
    )
    .fetch_one(database.database.pool())
    .await?;
    let envelope: EventEnvelope = serde_json::from_value(payload)?;
    let fact = envelope.payload_as::<ContentDocumentExtracted>()?;
    fact.validate()?;
    assert_eq!(fact.document, document);
    assert_eq!(fact.document_blob, ir_blob);
    assert_eq!(envelope.producer.as_str(), "ratatoskr-extractor");
    assert_eq!(
        envelope.aggregate_id.to_string(),
        format!("document:{document_id}")
    );
    assert_eq!(
        envelope
            .tenant_id
            .ok_or("the fact must carry its tenant")?
            .to_string(),
        format!("user:{tenant}")
    );
    assert_eq!(envelope.correlation_id.to_string(), correlation);
    assert_eq!(
        envelope
            .causation_id
            .ok_or("the fact must carry its causation")?
            .to_string(),
        format!("command:{command_id}")
    );

    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_claimed_run_exposes_the_owner_and_no_document_id()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let capture = CaptureCommandJson::url("https://example.test/article")
        .with_idempotency("capture-owner-only");
    consume_capture(pool, SUBJECT, &capture.to_bytes()).await?;
    let run = claim_queued_run(pool, "test-worker", 60, MAX_EVENT_BYTES)
        .await?
        .ok_or("the command did not produce queued work")?;

    assert_eq!(run.owner, UserId(capture.tenant_user));
    let identity_columns: i64 = sqlx::query_scalar(
        "select count(*) from information_schema.columns
          where table_schema = 'extractor' and table_name = 'extraction_runs'
            and column_name = 'document_id'",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(
        identity_columns, 0,
        "a run must not carry a document identity"
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn two_operations_over_one_digest_share_one_document()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let first = CaptureCommandJson::url("https://example.test/article")
        .with_idempotency("capture-share-one");
    let mut second = CaptureCommandJson::url("https://example.test/article")
        .with_idempotency("capture-share-two");
    second.tenant_user = first.tenant_user;
    for capture in [&first, &second] {
        consume_capture(pool, SUBJECT, &capture.to_bytes()).await?;
    }
    let source = blob_ref("text/html", 17, 'a')?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    for _ in 0..2 {
        let run = claim_queued_run(pool, "test-worker", 60, MAX_EVENT_BYTES)
            .await?
            .ok_or("the command did not produce queued work")?;
        let document = document(source.clone(), run.owner)?;
        let ir_blob = store_document_ir(&store, &document).await?;
        let fetch = CompletedFetch {
            final_url: "https://example.test/article",
            http_status: 200,
            media_type: "text/html",
            wire_bytes: 17,
            decoded_bytes: 17,
            attempts: 1,
            cache_outcome: "fresh",
            etag: None,
            last_modified: None,
            raw_blob: &source,
        };
        assert_eq!(
            complete_document(
                pool,
                &run,
                &document,
                &ir_blob,
                &fetch,
                &candidate_decisions(),
                &[],
            )
            .await?,
            Completion::Applied
        );
    }

    let payloads: Vec<serde_json::Value> = sqlx::query_scalar(
        "select payload from extractor.outbox_events
          where subject = 'evt.content.document.extracted.v1'",
    )
    .fetch_all(pool)
    .await?;
    let mut events = Vec::new();
    for payload in payloads {
        let envelope: EventEnvelope = serde_json::from_value(payload)?;
        let fact = envelope.payload_as::<ContentDocumentExtracted>()?;
        events.push((envelope.event_id, fact.document));
    }
    let [
        (first_event, first_document),
        (second_event, second_document),
    ] = events.as_slice()
    else {
        return Err(format!("expected two document events, found {}", events.len()).into());
    };
    assert_ne!(
        first_event, second_event,
        "each operation has its own event"
    );
    assert_eq!(first_document.document_id, second_document.document_id);
    assert_eq!(
        first_document.content_digest,
        second_document.content_digest
    );

    let reports = reported_operations(pool).await?;
    assert_eq!(reports.len(), 2, "each operation reports on its own");
    let mut operations: Vec<_> = reports.iter().map(|report| report.operation_id.0).collect();
    operations.sort();
    operations.dedup();
    assert_eq!(operations.len(), 2);
    for report in &reports {
        let [result] = report.results.as_slice() else {
            return Err("a report must name the one extracted document".into());
        };
        assert_eq!(result.target, first_document.document_id.as_entity_ref());
    }
    database.cleanup().await?;
    Ok(())
}

/// The warning an oversize document leaves on an otherwise successful operation.
const NOT_INGESTED_MESSAGE: &str =
    "The document was extracted, but it is too large to add to the library.";

/// A document whose serialized event envelope is larger than the smallest accepted limit.
const LARGE_TEXT_BYTES: usize = 80_000;
const SMALLEST_EVENT_LIMIT: usize = 65_536;

struct Completed {
    completion: Completion,
    document: Document,
    ir_blob: BlobRef,
    run_id: uuid::Uuid,
}

/// Completes one run of a large document under `max_event_bytes`.
async fn complete_large_document(
    database: &TestDatabase,
    key: &str,
    max_event_bytes: usize,
) -> Result<Completed, Box<dyn std::error::Error>> {
    let pool = database.database.pool();
    consume_capture(
        pool,
        SUBJECT,
        &CaptureCommandJson::url("https://example.test/article")
            .with_idempotency(key)
            .to_bytes(),
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60, max_event_bytes)
        .await?
        .ok_or("the command did not produce queued work")?;
    let source = blob_ref("text/html", 17, 'a')?;
    let document = document_with_text(
        source.clone(),
        run.owner,
        &"a long paragraph ".repeat(LARGE_TEXT_BYTES / 17),
    )?;
    let root = TemporaryBlobRoot::create().await?;
    let ir_blob = store_document_ir(&BlobStore::new(root.path()), &document).await?;
    let fetch = CompletedFetch {
        final_url: "https://example.test/article",
        http_status: 200,
        media_type: "text/html",
        wire_bytes: 17,
        decoded_bytes: 17,
        attempts: 1,
        cache_outcome: "fresh",
        etag: None,
        last_modified: None,
        raw_blob: &source,
    };
    let completion = complete_document(
        pool,
        &run,
        &document,
        &ir_blob,
        &fetch,
        &candidate_decisions(),
        &[],
    )
    .await?;
    Ok(Completed {
        completion,
        document,
        ir_blob,
        run_id: run.run_id,
    })
}

/// The terminal operation reports in the outbox.
async fn reported_operations(
    pool: &sqlx::PgPool,
) -> Result<Vec<OperationReported>, Box<dyn std::error::Error>> {
    let payloads: Vec<serde_json::Value> = sqlx::query_scalar(
        "select payload from extractor.outbox_events
          where subject = 'evt.platform.operation.reported.v1'",
    )
    .fetch_all(pool)
    .await?;
    // Intake enqueues the non-terminal acceptance report; only the terminal one is under test.
    let mut terminal = Vec::new();
    for payload in payloads {
        let envelope: EventEnvelope = serde_json::from_value(payload)?;
        let report = envelope.payload_as::<OperationReported>()?;
        if report.status.is_terminal() {
            terminal.push(report);
        }
    }
    Ok(terminal)
}

/// XR-021 CONTRACTS.md R3-14 rule 2: the public code and safe message of each closed failure
/// class, the `retryable` value the caller passes, and the class names that stay operator-only.
const PUBLIC_FAILURES: [(&str, bool, &str, &str); 11] = [
    (
        "pdf_no_text_layer",
        false,
        "content.extraction.no_text_layer",
        "The PDF has no text layer; scanned or image-only PDFs are not supported yet.",
    ),
    (
        "pdf_encrypted",
        false,
        "content.extraction.encrypted",
        "The PDF is encrypted.",
    ),
    (
        "unsupported_media",
        false,
        "content.extraction.unsupported_media",
        "This file type cannot be extracted.",
    ),
    (
        "blob_owner",
        false,
        "content.extraction.source_unavailable",
        "The uploaded file could not be read.",
    ),
    (
        "blob_missing",
        false,
        "content.extraction.source_unavailable",
        "The uploaded file could not be read.",
    ),
    (
        "blob_unreadable",
        false,
        "content.extraction.source_unavailable",
        "The uploaded file could not be read.",
    ),
    (
        "blob_mismatch",
        false,
        "content.extraction.source_unavailable",
        "The uploaded file could not be read.",
    ),
    (
        "parse",
        false,
        "content.extraction.failed",
        "The document could not be extracted.",
    ),
    (
        "fetch",
        true,
        "content.extraction.failed",
        "The document could not be extracted.",
    ),
    (
        "artifact",
        false,
        "content.extraction.failed",
        "The document could not be extracted.",
    ),
    (
        "a_class_nobody_defined",
        false,
        "content.extraction.failed",
        "The document could not be extracted.",
    ),
];

#[tokio::test]
async fn each_failure_class_reports_its_public_code() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    for (class, retryable, code, message) in PUBLIC_FAILURES {
        let capture = CaptureCommandJson::url("https://example.test/article");
        consume_capture(pool, SUBJECT, &capture.to_bytes()).await?;
        let run = claim_queued_run(pool, "test-worker", 60, MAX_EVENT_BYTES)
            .await?
            .ok_or("the command did not produce queued work")?;

        assert_eq!(
            fail_run(pool, run.run_id, class, retryable, &[]).await?,
            Completion::Applied
        );

        let reports = reported_operations(pool).await?;
        let report = reports
            .iter()
            .find(|report| report.operation_id.0 == capture.operation_id)
            .ok_or("the failed run enqueued no terminal report")?;
        let error = report
            .error
            .as_ref()
            .ok_or("a failed report carries an error")?;
        assert_eq!(error.code.as_str(), code, "code for class {class}");
        assert_eq!(error.message.as_str(), message, "message for class {class}");
        assert_eq!(error.retryable, retryable, "retryable for class {class}");
        assert!(
            !error.message.as_str().contains(class),
            "the message must not name the class {class}"
        );
    }
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn an_oversize_document_is_reported_partial_and_no_event_is_enqueued()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    let completed = complete_large_document(&database, "oversize", SMALLEST_EVENT_LIMIT).await?;
    assert_eq!(completed.completion, Completion::Applied);

    let document_events: i64 = sqlx::query_scalar(
        "select count(*) from extractor.outbox_events
          where subject = 'evt.content.document.extracted.v1'",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(document_events, 0, "an oversize fact must not be enqueued");

    let reports = reported_operations(pool).await?;
    let [report] = reports.as_slice() else {
        return Err(format!("expected one operation report, found {}", reports.len()).into());
    };
    assert_eq!(report.status, OperationStatus::PartiallySucceeded);
    let [warning] = report.warnings.as_slice() else {
        return Err("expected exactly one warning".into());
    };
    assert_eq!(warning.code.as_str(), "content.document.not_ingested");
    assert_eq!(warning.message.as_str(), NOT_INGESTED_MESSAGE);
    assert!(report.error.is_none());
    report.validate()?;
    let [result] = report.results.as_slice() else {
        return Err("the extracted document must stay in the results".into());
    };
    assert_eq!(result.result_kind.as_str(), "content.document");
    assert_eq!(
        result.target,
        completed.document.document_id.as_entity_ref()
    );
    assert_eq!(result.blob.as_ref(), Some(&completed.ir_blob));

    // The extraction itself succeeded and is recorded as such.
    let status: String =
        sqlx::query_scalar("select status from extractor.extraction_runs where run_id = $1")
            .bind(completed.run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(status, "succeeded");
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_document_under_the_limit_still_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let pool = database.database.pool();
    // The very document the oversize test refuses, under the production default limit.
    let completed = complete_large_document(&database, "under-limit", MAX_EVENT_BYTES).await?;
    assert_eq!(completed.completion, Completion::Applied);

    let document_events: i64 = sqlx::query_scalar(
        "select count(*) from extractor.outbox_events
          where subject = 'evt.content.document.extracted.v1'",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(document_events, 1);
    let reports = reported_operations(pool).await?;
    let [report] = reports.as_slice() else {
        return Err("expected one operation report".into());
    };
    assert_eq!(report.status, OperationStatus::Succeeded);
    assert!(report.warnings.is_empty());
    report.validate()?;
    database.cleanup().await?;
    Ok(())
}

fn candidate_decisions() -> Vec<CandidateDecision> {
    ["semantic", "readability", "density"]
        .into_iter()
        .map(|strategy| CandidateDecision {
            strategy: strategy.to_owned(),
            blocks: Vec::new(),
            metrics: QualityMetrics {
                text_characters: 120,
                paragraph_count: 2,
                text_volume: 100,
                paragraph_distribution: 100,
                non_link_share: 200,
                non_boilerplate_share: 200,
                title_agreement: 100,
            },
            score: 700,
            evaluator_version: "quality_v1",
            reasons: vec![QualityReason::Accepted],
            accepted: true,
            selected: strategy == "semantic",
        })
        .collect()
}

async fn verify_completion(
    pool: &sqlx::PgPool,
    run_id: uuid::Uuid,
    operation_id: uuid::Uuid,
    document: &Document,
    ir_blob: &BlobRef,
) -> Result<(), Box<dyn std::error::Error>> {
    let row = sqlx::query("select status from extractor.extraction_runs where run_id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await?;
    assert_eq!(row.try_get::<String, _>("status")?, "succeeded");
    let artifacts: i64 = sqlx::query_scalar(
        "select count(*) from extractor.artifacts where run_id = $1 and kind = 'document_ir'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(artifacts, 1);
    let fetches: i64 =
        sqlx::query_scalar("select count(*) from extractor.fetches where run_id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(fetches, 1);

    let rows = sqlx::query(
        "select subject, payload from extractor.outbox_events
          where subject = 'evt.content.document.extracted.v1'
             or payload->'payload'->>'status' = 'succeeded'
          order by subject",
    )
    .fetch_all(pool)
    .await?;
    assert_eq!(rows.len(), 2);
    let document_row = rows.first().ok_or("document event is missing")?;
    let report_row = rows.last().ok_or("operation report is missing")?;
    let document_envelope: EventEnvelope =
        serde_json::from_value(document_row.try_get("payload")?)?;
    let emitted = document_envelope.payload_as::<ContentDocumentExtracted>()?;
    assert_eq!(&emitted.document, document);
    assert_eq!(&emitted.document_blob, ir_blob);
    let report_envelope: EventEnvelope = serde_json::from_value(report_row.try_get("payload")?)?;
    let report = report_envelope.payload_as::<OperationReported>()?;
    assert_eq!(report.status, OperationStatus::Succeeded);
    assert_eq!(report.operation_id.to_string(), operation_id.to_string());
    assert_eq!(report.results.len(), 1);
    let result = report
        .results
        .first()
        .ok_or("operation result is missing")?;
    assert_eq!(result.target, document.document_id.as_entity_ref());
    assert_eq!(result.blob.as_ref(), Some(ir_blob));
    Ok(())
}

fn document(source_blob: BlobRef, owner: UserId) -> Result<Document, Box<dyn std::error::Error>> {
    document_with_text(source_blob, owner, "One document.")
}

fn document_with_text(
    source_blob: BlobRef,
    owner: UserId,
    text: &str,
) -> Result<Document, Box<dyn std::error::Error>> {
    let blocks = vec![DocumentBlock::Paragraph {
        block_id: BlockId::parse("018f0000-0000-7000-8000-000000000099")?,
        text: text.to_owned(),
    }];
    let digest = format!(
        "{:x}",
        Sha256::digest(ratatoskr_identifiers::canonical_json(&blocks)?.as_bytes())
    );
    let content_digest = ContentDigest {
        algorithm: DigestAlgorithm::Sha256,
        hex: DigestHex::parse(&digest)?,
    };
    Ok(Document {
        document_id: stable_document_id(owner, &content_digest),
        source_address: DocumentAddress::parse("https://example.test/article")?,
        content_digest,
        title: Some("Article".to_owned()),
        language: None,
        blocks,
        provenance: vec![DocumentProvenance {
            block_index: 0,
            extraction_strategy: ExtractionStrategy::parse("html_primitives")?,
            source_blob,
        }],
    })
}

fn blob_ref(
    media_type: &str,
    length_bytes: u64,
    digit: char,
) -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&digit.to_string().repeat(64))?,
        },
        media_type: MediaType::parse(media_type)?,
        length_bytes,
    })
}
