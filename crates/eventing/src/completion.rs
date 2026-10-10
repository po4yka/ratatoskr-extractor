//! Terminal run states: failure, completed documents and the outbox facts they produce.

use extractor_blob_store::BlobStore;
use extractor_document_ir::CandidateDecision;
use ratatoskr_document_contracts::{ContentDocumentExtracted, Document};
use ratatoskr_error_contracts::{ErrorCode, ErrorEnvelope};
use ratatoskr_event_envelope::{
    EnvelopeSchemaVersion, EventEnvelope, EventPayload as _, ProducerName,
};
use ratatoskr_identifiers::{
    BlobRef, EntityRef, EventId, Extensions, OperationId, SafeMessage, TenantRef, WireTimestamp,
};
use ratatoskr_operation_contracts::{
    OperationReported, OperationResultKind, OperationResultRef, OperationStatus,
};
use sqlx::{PgPool, PgTransaction};

use crate::terminal::{self, ResolutionStep};
use crate::{CompletedFetch, Completion, ConsumeError, PRODUCER, REPORT_SUBJECT};

/// Atomically records a terminal safe failure and its operation report.
///
/// # Errors
///
/// Returns [`ConsumeError`] when the run is not executing or persistence fails.
pub async fn fail_run(
    pool: &PgPool,
    run_id: uuid::Uuid,
    failure_class: &str,
    retryable: bool,
    steps: &[ResolutionStep<'_>],
) -> Result<Completion, ConsumeError> {
    if failure_class.is_empty() || failure_class.len() > 64 {
        return Err(ConsumeError::InvalidRunState);
    }
    let mut transaction = pool.begin().await?;
    let context = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, uuid::Uuid, String)>(
        "update extractor.extraction_runs
            set status = 'failed', completed_at = transaction_timestamp(),
                last_error_class = $2, claimed_until = null, claimed_by = null
          where run_id = $1 and status = 'running'
          returning command_id, operation_id, owner_id, correlation_id",
    )
    .bind(run_id)
    .bind(failure_class)
    .fetch_optional(&mut *transaction)
    .await?
    .map(CompletionContext::from);
    let Some(context) = context else {
        transaction.commit().await?;
        return Ok(Completion::Duplicate);
    };
    terminal::insert_resolution_steps(&mut transaction, run_id, steps).await?;
    enqueue_failed_report(&mut transaction, &context, retryable).await?;
    transaction.commit().await?;
    Ok(Completion::Applied)
}

async fn enqueue_failed_report(
    transaction: &mut PgTransaction<'_>,
    context: &CompletionContext,
    retryable: bool,
) -> Result<(), ConsumeError> {
    let operation_id = OperationId(context.operation);
    let correlation_id = EntityRef::parse(&context.correlation)?;
    let tenant_id = TenantRef::parse(&format!("user:{}", context.owner))?;
    let mut error = ErrorEnvelope::new(
        ErrorCode::parse("content.extraction.failed")?,
        SafeMessage::parse("The document could not be extracted.")?,
        retryable,
    );
    error.correlation_id = Some(correlation_id.clone());
    let mut envelope = EventEnvelope {
        event_id: EventId::new_v7(),
        event_type: OperationReported::event_type(),
        occurred_at: WireTimestamp::now(),
        producer: ProducerName::parse(PRODUCER)?,
        aggregate_id: operation_id.as_entity_ref(),
        correlation_id,
        causation_id: Some(EntityRef::parse(&format!("command:{}", context.command))?),
        tenant_id: Some(tenant_id),
        schema_version: EnvelopeSchemaVersion::CURRENT,
        payload: serde_json::Map::new(),
        extensions: Extensions::new(),
    };
    envelope.set_payload(&OperationReported {
        operation_id,
        status: OperationStatus::Failed,
        stage: None,
        progress_percent: None,
        results: Vec::new(),
        error: Some(error),
        warnings: Vec::new(),
        extensions: Extensions::new(),
    })?;
    enqueue_event(transaction, context, REPORT_SUBJECT, &envelope).await
}

/// Atomically commits one completed document and its two event facts.
///
/// # Errors
///
/// Returns [`ConsumeError`] when the run or terminal records cannot be validated or persisted.
pub async fn complete_document(
    pool: &PgPool,
    run_id: uuid::Uuid,
    document: &Document,
    ir_blob: &BlobRef,
    fetch: &CompletedFetch<'_>,
    candidates: &[CandidateDecision],
    steps: &[ResolutionStep<'_>],
) -> Result<Completion, ConsumeError> {
    complete(
        pool,
        run_id,
        &Success {
            document,
            ir_blob,
            raw_blob: fetch.raw_blob,
            fetch: Some(fetch),
            candidates,
            steps,
        },
    )
    .await
}

/// Atomically commits one document extracted from a peer-owned blob and its two event facts.
///
/// No fetch happened, so no `extractor.fetches` row is written; `raw_blob` is the extractor-owned
/// copy of the peer bytes.
///
/// # Errors
///
/// Returns [`ConsumeError`] when the run or terminal records cannot be validated or persisted.
pub async fn complete_blob_document(
    pool: &PgPool,
    run_id: uuid::Uuid,
    document: &Document,
    ir_blob: &BlobRef,
    raw_blob: &BlobRef,
    candidates: &[CandidateDecision],
    steps: &[ResolutionStep<'_>],
) -> Result<Completion, ConsumeError> {
    complete(
        pool,
        run_id,
        &Success {
            document,
            ir_blob,
            raw_blob,
            fetch: None,
            candidates,
            steps,
        },
    )
    .await
}

/// Everything a successful terminal transition commits.
struct Success<'a> {
    document: &'a Document,
    ir_blob: &'a BlobRef,
    raw_blob: &'a BlobRef,
    fetch: Option<&'a CompletedFetch<'a>>,
    candidates: &'a [CandidateDecision],
    steps: &'a [ResolutionStep<'a>],
}

async fn complete(
    pool: &PgPool,
    run_id: uuid::Uuid,
    success: &Success<'_>,
) -> Result<Completion, ConsumeError> {
    let Success {
        document,
        ir_blob,
        raw_blob,
        fetch,
        candidates,
        steps,
    } = *success;
    require_owned_sha256(ir_blob)?;
    require_owned_sha256(raw_blob)?;
    let length = i64::try_from(ir_blob.length_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
    let raw_length =
        i64::try_from(raw_blob.length_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
    terminal::validate_candidates(candidates, 1)?;
    let mut transaction = pool.begin().await?;
    let context = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, uuid::Uuid, String)>(
        "update extractor.extraction_runs
            set status = 'succeeded', completed_at = transaction_timestamp(),
                claimed_until = null, claimed_by = null
          where run_id = $1 and status = 'running' and document_id = $2
          returning command_id, operation_id, owner_id, correlation_id",
    )
    .bind(run_id)
    .bind(document.document_id.0)
    .fetch_optional(&mut *transaction)
    .await?
    .map(CompletionContext::from);
    let Some(context) = context else {
        let status: Option<String> =
            sqlx::query_scalar("select status from extractor.extraction_runs where run_id = $1")
                .bind(run_id)
                .fetch_optional(&mut *transaction)
                .await?;
        transaction.commit().await?;
        return match status.as_deref() {
            Some("succeeded") => Ok(Completion::Duplicate),
            _ => Err(ConsumeError::InvalidRunState),
        };
    };

    if let Some(fetch) = fetch {
        terminal::insert_fetch(&mut transaction, run_id, fetch).await?;
    }
    insert_artifact(&mut transaction, run_id, "raw_source", raw_blob, raw_length).await?;
    insert_artifact(&mut transaction, run_id, "document_ir", ir_blob, length).await?;

    terminal::insert_candidates(&mut transaction, run_id, candidates).await?;
    terminal::insert_resolution_steps(&mut transaction, run_id, steps).await?;
    enqueue_completion_events(&mut transaction, &context, document, ir_blob).await?;
    transaction.commit().await?;
    Ok(Completion::Applied)
}

/// Records a bounded quality failure under the extraction path's explicit failure class.
///
/// # Errors
///
/// Returns [`ConsumeError`] when the class is invalid or terminal persistence fails.
pub async fn reject_quality(
    pool: &PgPool,
    run_id: uuid::Uuid,
    fetch: &CompletedFetch<'_>,
    candidates: &[CandidateDecision],
    failure_class: &str,
    steps: &[ResolutionStep<'_>],
) -> Result<Completion, ConsumeError> {
    reject(
        pool,
        run_id,
        &Rejection {
            raw_blob: fetch.raw_blob,
            fetch: Some(fetch),
            candidates,
            failure_class,
            steps,
        },
    )
    .await
}

/// Records a bounded quality failure of a peer-owned blob run.
///
/// No fetch happened, so no `extractor.fetches` row is written; `raw_blob` is the extractor-owned
/// copy of the peer bytes and stays as evidence.
///
/// # Errors
///
/// Returns [`ConsumeError`] when the class is invalid or terminal persistence fails.
pub async fn reject_blob_quality(
    pool: &PgPool,
    run_id: uuid::Uuid,
    raw_blob: &BlobRef,
    candidates: &[CandidateDecision],
    failure_class: &str,
    steps: &[ResolutionStep<'_>],
) -> Result<Completion, ConsumeError> {
    reject(
        pool,
        run_id,
        &Rejection {
            raw_blob,
            fetch: None,
            candidates,
            failure_class,
            steps,
        },
    )
    .await
}

/// Everything a quality-rejected terminal transition commits.
struct Rejection<'a> {
    raw_blob: &'a BlobRef,
    fetch: Option<&'a CompletedFetch<'a>>,
    candidates: &'a [CandidateDecision],
    failure_class: &'a str,
    steps: &'a [ResolutionStep<'a>],
}

async fn reject(
    pool: &PgPool,
    run_id: uuid::Uuid,
    rejection: &Rejection<'_>,
) -> Result<Completion, ConsumeError> {
    let Rejection {
        raw_blob,
        fetch,
        candidates,
        failure_class,
        steps,
    } = *rejection;
    if failure_class.is_empty() || failure_class.len() > 64 {
        return Err(ConsumeError::InvalidRunState);
    }
    require_owned_sha256(raw_blob)?;
    terminal::validate_candidates(candidates, 0)?;
    let raw_length =
        i64::try_from(raw_blob.length_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
    let mut transaction = pool.begin().await?;
    let context = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, uuid::Uuid, String)>(
        "update extractor.extraction_runs
            set status = 'failed', completed_at = transaction_timestamp(),
                last_error_class = $2, claimed_until = null, claimed_by = null
          where run_id = $1 and status = 'running'
          returning command_id, operation_id, owner_id, correlation_id",
    )
    .bind(run_id)
    .bind(failure_class)
    .fetch_optional(&mut *transaction)
    .await?
    .map(CompletionContext::from);
    let Some(context) = context else {
        transaction.commit().await?;
        return Ok(Completion::Duplicate);
    };
    if let Some(fetch) = fetch {
        terminal::insert_fetch(&mut transaction, run_id, fetch).await?;
    }
    insert_artifact(&mut transaction, run_id, "raw_source", raw_blob, raw_length).await?;
    terminal::insert_candidates(&mut transaction, run_id, candidates).await?;
    terminal::insert_resolution_steps(&mut transaction, run_id, steps).await?;
    enqueue_failed_report(&mut transaction, &context, false).await?;
    transaction.commit().await?;
    Ok(Completion::Applied)
}

/// An artifact reference is extractor-owned and uses the only supported digest algorithm.
fn require_owned_sha256(reference: &BlobRef) -> Result<(), ConsumeError> {
    if reference.owner_service.as_str() == PRODUCER
        && matches!(
            reference.digest.algorithm,
            ratatoskr_identifiers::DigestAlgorithm::Sha256
        )
    {
        Ok(())
    } else {
        Err(ConsumeError::InvalidArtifact)
    }
}

/// Persists one fetch row and its raw-source artifact without touching run state.
///
/// Resolution steps persist additional fetch rows while another completion path owns the
/// terminal transition, so this helper validates artifact ownership only and never updates the
/// run row itself.
///
/// # Errors
///
/// Returns [`ConsumeError`] when artifact ownership is invalid or persistence fails.
pub async fn record_fetch(
    pool: &PgPool,
    run_id: uuid::Uuid,
    fetch: &CompletedFetch<'_>,
) -> Result<(), ConsumeError> {
    require_owned_sha256(fetch.raw_blob)?;
    let raw_length =
        i64::try_from(fetch.raw_blob.length_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
    let mut transaction = pool.begin().await?;
    terminal::insert_fetch(&mut transaction, run_id, fetch).await?;
    insert_artifact(
        &mut transaction,
        run_id,
        "raw_source",
        fetch.raw_blob,
        raw_length,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn insert_artifact(
    transaction: &mut PgTransaction<'_>,
    run_id: uuid::Uuid,
    kind: &str,
    reference: &BlobRef,
    length: i64,
) -> Result<(), ConsumeError> {
    sqlx::query(
        "insert into extractor.artifacts
             (artifact_id, run_id, kind, owner_service, digest_algorithm, digest_hex, media_type,
              length_bytes, created_at)
          values ($1, $2, $3, $4, 'sha256', $5, $6, $7,
                  transaction_timestamp())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(run_id)
    .bind(kind)
    .bind(PRODUCER)
    .bind(reference.digest.hex.as_str())
    .bind(reference.media_type.as_str())
    .bind(length)
    .execute(&mut **transaction)
    .await?;

    Ok(())
}

/// Stores the canonical shared Document IR bytes in the extractor-owned blob store.
///
/// # Errors
///
/// Returns [`ConsumeError`] when serialization or local content-addressed storage fails.
pub async fn store_document_ir(
    store: &BlobStore,
    document: &Document,
) -> Result<BlobRef, ConsumeError> {
    let canonical = ratatoskr_identifiers::canonical_json(document)?;
    store
        .store(
            "application/json",
            futures_util::stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from(canonical))]),
        )
        .await
        .map_err(ConsumeError::ArtifactStore)
}

type CompletionRow = (uuid::Uuid, uuid::Uuid, uuid::Uuid, String);

struct CompletionContext {
    command: uuid::Uuid,
    operation: uuid::Uuid,
    owner: uuid::Uuid,
    correlation: String,
}

impl From<CompletionRow> for CompletionContext {
    fn from((command, operation, owner, correlation): CompletionRow) -> Self {
        Self {
            command,
            operation,
            owner,
            correlation,
        }
    }
}

async fn enqueue_completion_events(
    transaction: &mut PgTransaction<'_>,
    context: &CompletionContext,
    document: &Document,
    ir_blob: &BlobRef,
) -> Result<(), ConsumeError> {
    let operation_id = OperationId(context.operation);
    let correlation_id = EntityRef::parse(&context.correlation)?;
    let tenant_id = TenantRef::parse(&format!("user:{}", context.owner))?;
    let causation_id = EntityRef::parse(&format!("command:{}", context.command))?;

    let fact = ContentDocumentExtracted {
        document: document.clone(),
        document_blob: ir_blob.clone(),
        extensions: Extensions::new(),
    };
    fact.validate().map_err(|_| ConsumeError::InvalidArtifact)?;
    let mut document_event = EventEnvelope {
        event_id: EventId::new_v7(),
        event_type: ContentDocumentExtracted::event_type(),
        occurred_at: WireTimestamp::now(),
        producer: ProducerName::parse(PRODUCER)?,
        aggregate_id: document.document_id.as_entity_ref(),
        correlation_id: correlation_id.clone(),
        causation_id: Some(causation_id.clone()),
        tenant_id: Some(tenant_id),
        schema_version: EnvelopeSchemaVersion::CURRENT,
        payload: serde_json::Map::new(),
        extensions: Extensions::new(),
    };
    document_event.set_payload(&fact)?;
    enqueue_event(
        transaction,
        context,
        "evt.content.document.extracted.v1",
        &document_event,
    )
    .await?;

    let event_id = EventId::new_v7();
    let mut report_event = EventEnvelope {
        event_id,
        event_type: OperationReported::event_type(),
        occurred_at: WireTimestamp::now(),
        producer: ProducerName::parse(PRODUCER)?,
        aggregate_id: operation_id.as_entity_ref(),
        correlation_id,
        causation_id: Some(causation_id),
        tenant_id: Some(tenant_id),
        schema_version: EnvelopeSchemaVersion::CURRENT,
        payload: serde_json::Map::new(),
        extensions: Extensions::new(),
    };
    report_event.set_payload(&OperationReported {
        operation_id,
        status: OperationStatus::Succeeded,
        stage: None,
        progress_percent: None,
        results: vec![OperationResultRef {
            result_kind: OperationResultKind::parse("content.document")?,
            target: document.document_id.as_entity_ref(),
            blob: Some(ir_blob.clone()),
            ai_archive_import_summary: None,
            extensions: Extensions::new(),
        }],
        error: None,
        warnings: Vec::new(),
        extensions: Extensions::new(),
    })?;
    enqueue_event(transaction, context, REPORT_SUBJECT, &report_event).await
}

async fn enqueue_event(
    transaction: &mut PgTransaction<'_>,
    context: &CompletionContext,
    subject: &str,
    envelope: &EventEnvelope,
) -> Result<(), ConsumeError> {
    sqlx::query(
        "insert into extractor.outbox_events
             (outbox_id, message_id, causation_command_id, operation_id, subject, payload,
              enqueued_at, next_attempt_at)
         values ($1, $2, $3, $4, $5, $6, transaction_timestamp(), transaction_timestamp())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(envelope.event_id.0)
    .bind(context.command)
    .bind(context.operation)
    .bind(subject)
    .bind(serde_json::to_value(envelope)?)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
