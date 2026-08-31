//! Atomic successful extraction completion.

use extractor_blob_store::BlobStore;
use extractor_document_ir::{CandidateDecision, QualityMetrics, QualityReason};
use extractor_eventing::{
    CompletedFetch, Completion, complete_document, consume_capture, store_document_ir,
};
use extractor_persistence::test_support::TestDatabase;
use extractor_test_support::TemporaryBlobRoot;
use ratatoskr_document_contracts::{
    Document, DocumentAddress, DocumentBlock, DocumentProvenance, ExtractionStrategy,
};
use ratatoskr_event_envelope::{
    EnvelopeSchemaVersion, EventEnvelope, EventPayload as _, ProducerName,
};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, BlockId, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, EntityRef,
    EventId, Extensions, MediaType, TenantRef, WireTimestamp,
};
use ratatoskr_operation_contracts::{OperationReported, OperationStatus};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;

const SUBJECT: &str = "cmd.content.capture.requested.v1";

#[test]
fn document_completion_uses_registered_typed_event_without_wire_drift()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = include_str!("fixtures/document_extracted_existing_wire.json");
    let document: Document = serde_json::from_str(fixture)?;
    assert_eq!(
        ratatoskr_identifiers::canonical_json(&document)?,
        fixture,
        "the Extractor compatibility fixture drifted from the typed Document wire"
    );

    let mut envelope = EventEnvelope {
        event_id: EventId::parse("018f0000-0000-7000-8000-000000000001")?,
        event_type: Document::event_type(),
        occurred_at: WireTimestamp::parse("2026-08-21T10:00:00Z")?,
        producer: ProducerName::parse("ratatoskr-extractor")?,
        aggregate_id: document.document_id.as_entity_ref(),
        correlation_id: EntityRef::parse("operation:018f0000-0000-7000-8000-000000000002")?,
        causation_id: Some(EntityRef::parse(
            "command:018f0000-0000-7000-8000-000000000003",
        )?),
        tenant_id: Some(TenantRef::parse(
            "user:018f0000-0000-7000-8000-000000000004",
        )?),
        schema_version: EnvelopeSchemaVersion::CURRENT,
        payload: serde_json::Map::new(),
        extensions: Extensions::new(),
    };
    envelope.set_payload(&document)?;

    assert_eq!(Document::EVENT_TYPE, "content.document.extracted.v1");
    assert_eq!(envelope.event_type, Document::event_type());
    assert_eq!(envelope.aggregate_id, document.document_id.as_entity_ref());
    assert_eq!(envelope.payload_as::<Document>()?, document);
    assert_eq!(
        "evt.content.document.extracted.v1",
        format!("evt.{}", Document::EVENT_TYPE)
    );
    let implementation = include_str!("../src/lib.rs");
    assert!(
        implementation.contains("document_event.set_payload(document)?;"),
        "production still constructs the registered document event manually"
    );
    assert!(
        !implementation.contains("EventType::parse(\"content.document.extracted.v1\")"),
        "production still duplicates the registered event type string"
    );
    Ok(())
}

#[tokio::test]
async fn completed_document_and_report_commit_with_one_run()
-> Result<(), Box<dyn std::error::Error>> {
    let database = TestDatabase::create().await?;
    let operation_id = uuid::Uuid::now_v7();
    let command = serde_json::to_vec(&json!({
        "command_id": uuid::Uuid::now_v7(),
        "command_type": "content.capture.requested.v1",
        "requested_at": "2026-08-21T10:00:00Z",
        "operation_id": operation_id,
        "tenant_id": format!("user:{}", uuid::Uuid::now_v7()),
        "correlation_id": format!("operation:{operation_id}"),
        "idempotency_key": "capture-completed-once",
        "payload": { "url": "https://example.test/article" }
    }))?;
    consume_capture(database.database.pool(), SUBJECT, &command).await?;
    let (run_id, document_id): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
        "update extractor.extraction_runs set status = 'running', started_at = now()
         returning run_id, document_id",
    )
    .fetch_one(database.database.pool())
    .await?;
    let source = blob_ref("text/html", 17, 'a')?;
    let document = document(source.clone(), DocumentId(document_id))?;
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
            run_id,
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
            run_id,
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
    let row =
        sqlx::query("select status, document_id from extractor.extraction_runs where run_id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await?;
    assert_eq!(row.try_get::<String, _>("status")?, "succeeded");
    assert_eq!(
        row.try_get::<uuid::Uuid, _>("document_id")?.to_string(),
        document.document_id.to_string(),
    );
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
    assert_eq!(
        document_row.try_get::<String, _>("subject")?,
        "evt.content.document.extracted.v1"
    );
    let document_envelope: EventEnvelope =
        serde_json::from_value(document_row.try_get("payload")?)?;
    assert_eq!(document_envelope.event_type, Document::event_type());
    assert_eq!(
        document_envelope.aggregate_id,
        document.document_id.as_entity_ref()
    );
    let emitted = document_envelope.payload_as::<Document>()?;
    assert_eq!(&emitted, document);
    assert_eq!(
        ratatoskr_identifiers::canonical_json(&emitted)?.as_bytes(),
        ratatoskr_identifiers::canonical_json(document)?.as_bytes()
    );
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

fn document(
    source_blob: BlobRef,
    document_id: DocumentId,
) -> Result<Document, Box<dyn std::error::Error>> {
    let blocks = vec![DocumentBlock::Paragraph {
        block_id: BlockId::parse("018f0000-0000-7000-8000-000000000099")?,
        text: "One document.".to_owned(),
    }];
    let digest = format!(
        "{:x}",
        Sha256::digest(ratatoskr_identifiers::canonical_json(&blocks)?.as_bytes())
    );
    Ok(Document {
        document_id,
        source_address: DocumentAddress::parse("https://example.test/article")?,
        content_digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&digest)?,
        },
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
