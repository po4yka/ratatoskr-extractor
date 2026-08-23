#![forbid(unsafe_code)]

//! Extractor command inbox and transactional report outbox.

mod consumer;
mod outbox;
mod terminal;

pub use consumer::{ConsumerReport, run_command_consumer};
pub use outbox::{NatsPublisher, OutboxReport, PublishError, Publisher, run_outbox_once};

use extractor_blob_store::{BlobStore, BlobStoreError};
use extractor_document_ir::CandidateDecision;
use extractor_url_routing::{RoutingPolicy, SourceRoute, classify, normalize};
use ratatoskr_document_contracts::Document;
use ratatoskr_error_contracts::{ErrorCode, ErrorEnvelope};
use ratatoskr_event_envelope::{
    EnvelopeError, EnvelopeSchemaVersion, EventEnvelope, EventPayload as _, EventType, ProducerName,
};
use ratatoskr_identifiers::{
    BlobRef, DocumentId, EntityRef, EventId, Extensions, OperationId, SafeMessage, TenantRef,
    WireTimestamp,
};
use ratatoskr_operation_contracts::{
    OperationReported, OperationResultKind, OperationResultRef, OperationStatus,
};
use serde::Deserialize;
use sqlx::{PgPool, PgTransaction};

const CAPTURE_COMMAND_TYPE: &str = "content.capture.requested.v1";
const CAPTURE_SUBJECT: &str = "cmd.content.capture.requested.v1";
const REPORT_SUBJECT: &str = "evt.platform.operation.reported.v1";
const PRODUCER: &str = "ratatoskr-extractor";
const COMMAND_PRODUCER: &str = "ratatoskr-platform";

/// Result of consuming one capture command delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reception {
    /// The command was applied by this delivery.
    Applied,
    /// The command identifier was already present in the inbox.
    Duplicate,
}

/// Result of committing a terminal extraction result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// This call committed the terminal result.
    Applied,
    /// The run was already terminal.
    Duplicate,
}

/// Successful safe-fetch facts committed with a Document IR result.
#[derive(Debug)]
pub struct CompletedFetch<'a> {
    /// Redirect-resolved URL.
    pub final_url: &'a str,
    /// Final HTTP status.
    pub http_status: u16,
    /// Effective stored media type.
    pub media_type: &'a str,
    /// Encoded bytes observed.
    pub wire_bytes: u64,
    /// Decoded bytes stored.
    pub decoded_bytes: u64,
    /// Transport attempts used.
    pub attempts: u32,
    /// `fresh` or `revalidated`.
    pub cache_outcome: &'a str,
    /// Safe entity validator.
    pub etag: Option<&'a str>,
    /// Safe modification validator.
    pub last_modified: Option<&'a str>,
    /// Raw content-addressed source.
    pub raw_blob: &'a BlobRef,
}

/// One leased queued extraction run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedRun {
    /// Stable run identity.
    pub run_id: uuid::Uuid,
    /// Stable document identity assigned with the run.
    pub document_id: DocumentId,
    /// Normalized untrusted public URL.
    pub url: String,
    /// Source classification recorded at intake.
    pub classification: String,
}

/// Why a capture command could not be consumed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConsumeError {
    /// The delivery subject is not the capture-command subject.
    #[error("the delivery subject is not content.capture.requested.v1")]
    InvalidSubject,
    /// The envelope type disagrees with its delivery subject.
    #[error("the command type is not content.capture.requested.v1")]
    InvalidCommandType,
    /// The capture address is not an HTTP(S) URL.
    #[error("the capture URL scheme is unsupported")]
    InvalidUrlScheme,
    /// The command is not valid JSON for the published Platform shape.
    #[error("the capture command is malformed")]
    Command(#[from] serde_json::Error),
    /// A service-controlled envelope identity is invalid.
    #[error("the operation report identity is invalid")]
    Identity(#[from] ratatoskr_identifiers::IdentifierError),
    /// The typed operation report could not form an event envelope.
    #[error("the operation report envelope is invalid")]
    Envelope(#[from] EnvelopeError),
    /// A service-controlled event name is invalid.
    #[error("the event type is invalid")]
    EventType(#[from] ratatoskr_event_envelope::EventTypeError),
    /// The command could not be persisted.
    #[error("the capture command could not be persisted")]
    Database(#[from] sqlx::Error),
    /// The capture URL violates normalization policy.
    #[error("the capture URL is not allowed")]
    Url(#[from] extractor_url_routing::UrlError),
    /// The run does not exist or is not executing.
    #[error("the extraction run is not running")]
    InvalidRunState,
    /// The IR artifact reference is foreign or malformed.
    #[error("the document artifact reference is invalid")]
    InvalidArtifact,
    /// A typed payload unexpectedly did not serialize as an object.
    #[error("the event payload is not an object")]
    InvalidPayload,
    /// The local content-addressed store refused the canonical IR bytes.
    #[error("the document artifact could not be stored")]
    ArtifactStore(#[from] BlobStoreError),
}

#[derive(Debug, Deserialize)]
struct CaptureCommandWire {
    command_id: uuid::Uuid,
    command_type: String,
    requested_at: WireTimestamp,
    operation_id: OperationId,
    tenant_id: TenantRef,
    correlation_id: EntityRef,
    idempotency_key: String,
    payload: CapturePayload,
}

#[derive(Debug, Deserialize)]
struct CapturePayload {
    url: url::Url,
}

/// Validated Platform request to capture one public address.
#[derive(Debug, Clone)]
pub struct CaptureCommand {
    /// At-least-once deduplication key.
    pub command_id: uuid::Uuid,
    /// Platform operation whose work is requested.
    pub operation_id: OperationId,
    /// Owner of the requested content.
    pub tenant_id: TenantRef,
    /// Cross-process correlation reference.
    pub correlation_id: EntityRef,
    /// Caller-supplied idempotency key.
    pub idempotency_key: String,
    /// When Platform accepted the command.
    pub requested_at: WireTimestamp,
    /// Untrusted source address, syntactically parsed only.
    pub url: url::Url,
}

/// Decodes Platform's current capture-command wire shape.
///
/// Unknown additive envelope fields are ignored. URL destination policy remains the safe fetcher's
/// responsibility.
///
/// # Errors
///
/// Returns [`ConsumeError`] when the subject, command type, or typed members are invalid.
pub fn decode_capture(subject: &str, payload: &[u8]) -> Result<CaptureCommand, ConsumeError> {
    if subject != CAPTURE_SUBJECT {
        return Err(ConsumeError::InvalidSubject);
    }
    let command: CaptureCommandWire = serde_json::from_slice(payload)?;
    if command.command_type != CAPTURE_COMMAND_TYPE {
        return Err(ConsumeError::InvalidCommandType);
    }
    if !matches!(command.payload.url.scheme(), "http" | "https") {
        return Err(ConsumeError::InvalidUrlScheme);
    }
    Ok(CaptureCommand {
        command_id: command.command_id,
        operation_id: command.operation_id,
        tenant_id: command.tenant_id,
        correlation_id: command.correlation_id,
        idempotency_key: command.idempotency_key,
        requested_at: command.requested_at,
        url: command.payload.url,
    })
}

/// Consumes one capture command delivery.
///
/// The inbox claim, operation report, outbox row, and applied marker commit in one `PostgreSQL`
/// transaction. A repeated `command_id` performs no second effect.
///
/// # Errors
///
/// Returns [`ConsumeError`] when the subject, command, contract values, or transaction are invalid.
pub async fn consume_capture(
    pool: &PgPool,
    subject: &str,
    payload: &[u8],
) -> Result<Reception, ConsumeError> {
    let command = decode_capture(subject, payload)?;

    let mut transaction = pool.begin().await?;
    let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
        "insert into extractor.inbox_events
             (command_id, subject, command_type, producer, received_at)
         values ($1, $2, $3, $4, transaction_timestamp())
         on conflict (command_id) do nothing
         returning command_id",
    )
    .bind(command.command_id)
    .bind(subject)
    .bind(CAPTURE_COMMAND_TYPE)
    .bind(COMMAND_PRODUCER)
    .fetch_optional(&mut *transaction)
    .await?;

    if inserted.is_none() {
        transaction.commit().await?;
        return Ok(Reception::Duplicate);
    }

    queue_run(&mut transaction, &command).await?;
    enqueue_queued_report(&mut transaction, &command).await?;

    sqlx::query(
        "update extractor.inbox_events
            set applied_at = transaction_timestamp(), outcome = 'applied'
          where command_id = $1",
    )
    .bind(command.command_id)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    Ok(Reception::Applied)
}

/// Leases one queued run for bounded worker execution.
///
/// # Errors
///
/// Returns [`ConsumeError`] when `PostgreSQL` cannot claim work.
pub async fn claim_queued_run(
    pool: &PgPool,
    claimed_by: &str,
    lease_seconds: i32,
) -> Result<Option<QueuedRun>, ConsumeError> {
    let claimed = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, String, String)>(
        "with next as (
             select r.run_id, r.document_id, s.normalized_url, s.classification
               from extractor.extraction_runs r
               join extractor.sources s on s.source_id = r.source_id
              where r.status = 'queued'
                 or (r.status = 'running'
                     and (r.claimed_until is null or r.claimed_until <= clock_timestamp()))
              order by r.queued_at
              limit 1 for update of r skip locked
         )
         update extractor.extraction_runs r
            set status = 'running', started_at = coalesce(r.started_at, clock_timestamp()),
                claimed_by = $1,
                claimed_until = clock_timestamp() + make_interval(secs => $2)
           from next where r.run_id = next.run_id
          returning r.run_id, next.document_id, next.normalized_url, next.classification",
    )
    .bind(claimed_by)
    .bind(lease_seconds.clamp(1, 3_600))
    .fetch_optional(pool)
    .await?;
    Ok(
        claimed.map(|(run_id, document_id, url, classification)| QueuedRun {
            run_id,
            document_id: DocumentId(document_id),
            url,
            classification,
        }),
    )
}

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
    .map(
        |(command_id, operation_id, owner_id, correlation_id)| CompletionContext {
            command: command_id,
            operation: operation_id,
            owner: owner_id,
            correlation: correlation_id,
        },
    );
    let Some(context) = context else {
        transaction.commit().await?;
        return Ok(Completion::Duplicate);
    };
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
) -> Result<Completion, ConsumeError> {
    if ir_blob.owner_service.as_str() != PRODUCER
        || fetch.raw_blob.owner_service.as_str() != PRODUCER
        || !matches!(
            ir_blob.digest.algorithm,
            ratatoskr_identifiers::DigestAlgorithm::Sha256
        )
        || !matches!(
            fetch.raw_blob.digest.algorithm,
            ratatoskr_identifiers::DigestAlgorithm::Sha256
        )
    {
        return Err(ConsumeError::InvalidArtifact);
    }
    let length = i64::try_from(ir_blob.length_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
    let raw_length =
        i64::try_from(fetch.raw_blob.length_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
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
    .map(
        |(command_id, operation_id, owner_id, correlation_id)| CompletionContext {
            command: command_id,
            operation: operation_id,
            owner: owner_id,
            correlation: correlation_id,
        },
    );
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

    terminal::insert_fetch(&mut transaction, run_id, fetch).await?;

    insert_artifact(
        &mut transaction,
        run_id,
        "raw_source",
        fetch.raw_blob,
        raw_length,
    )
    .await?;

    insert_artifact(&mut transaction, run_id, "document_ir", ir_blob, length).await?;

    terminal::insert_candidates(&mut transaction, run_id, candidates).await?;
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
) -> Result<Completion, ConsumeError> {
    if failure_class.is_empty() || failure_class.len() > 64 {
        return Err(ConsumeError::InvalidRunState);
    }
    if fetch.raw_blob.owner_service.as_str() != PRODUCER
        || !matches!(
            fetch.raw_blob.digest.algorithm,
            ratatoskr_identifiers::DigestAlgorithm::Sha256
        )
    {
        return Err(ConsumeError::InvalidArtifact);
    }
    terminal::validate_candidates(candidates, 0)?;
    let raw_length =
        i64::try_from(fetch.raw_blob.length_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
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
    .map(
        |(command_id, operation_id, owner_id, correlation_id)| CompletionContext {
            command: command_id,
            operation: operation_id,
            owner: owner_id,
            correlation: correlation_id,
        },
    );
    let Some(context) = context else {
        transaction.commit().await?;
        return Ok(Completion::Duplicate);
    };
    terminal::insert_fetch(&mut transaction, run_id, fetch).await?;
    insert_artifact(
        &mut transaction,
        run_id,
        "raw_source",
        fetch.raw_blob,
        raw_length,
    )
    .await?;
    terminal::insert_candidates(&mut transaction, run_id, candidates).await?;
    enqueue_failed_report(&mut transaction, &context, false).await?;
    transaction.commit().await?;
    Ok(Completion::Applied)
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
                 transaction_timestamp())
         on conflict (run_id, kind) do update set
             digest_hex = excluded.digest_hex, media_type = excluded.media_type,
             length_bytes = excluded.length_bytes",
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

struct CompletionContext {
    command: uuid::Uuid,
    operation: uuid::Uuid,
    owner: uuid::Uuid,
    correlation: String,
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

    let serde_json::Value::Object(document_payload) = serde_json::to_value(document)? else {
        return Err(ConsumeError::InvalidPayload);
    };
    let document_event = EventEnvelope {
        event_id: EventId::new_v7(),
        event_type: EventType::parse("content.document.extracted.v1")?,
        occurred_at: WireTimestamp::now(),
        producer: ProducerName::parse(PRODUCER)?,
        aggregate_id: document.document_id.as_entity_ref(),
        correlation_id: correlation_id.clone(),
        causation_id: Some(causation_id.clone()),
        tenant_id: Some(tenant_id),
        schema_version: EnvelopeSchemaVersion::CURRENT,
        payload: document_payload,
        extensions: Extensions::new(),
    };
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

async fn queue_run(
    transaction: &mut PgTransaction<'_>,
    command: &CaptureCommand,
) -> Result<(), ConsumeError> {
    let normalized = normalize(command.url.as_str(), &routing_policy())?;
    let normalized_url = normalized.normalized().as_str();
    let host = normalized
        .normalized()
        .host_str()
        .ok_or(ConsumeError::InvalidUrlScheme)?;
    let route = classify(&normalized);
    let source_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "insert into extractor.sources
             (source_id, owner_id, original_url, normalized_url, canonical_url, host,
              classification, created_at)
         values ($1, $2, $3, $4, $4, $5, $6, transaction_timestamp())
         on conflict (owner_id, normalized_url) do update
             set canonical_url = excluded.canonical_url
         returning source_id",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(command.tenant_id.user_id().0)
    .bind(normalized.original())
    .bind(normalized_url)
    .bind(host)
    .bind(route_name(route))
    .fetch_one(&mut **transaction)
    .await?;

    sqlx::query(
        "insert into extractor.extraction_runs
             (run_id, command_id, operation_id, owner_id, correlation_id, source_id, document_id,
              status, policy_version, normalizer_version, parser_version, queued_at)
         values ($1, $2, $3, $4, $5, $6, $7, 'queued', 'ssrf-v1', 'url-v1', $8,
                 transaction_timestamp())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(command.command_id)
    .bind(command.operation_id.0)
    .bind(command.tenant_id.user_id().0)
    .bind(command.correlation_id.to_string())
    .bind(source_id)
    .bind(DocumentId::new_v7().0)
    .bind(parser_version(route))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Names the parser generation expected for a classified source at intake time.
const fn parser_version(route: SourceRoute) -> &'static str {
    match route {
        SourceRoute::Pdf => "pdf-v1",
        SourceRoute::HackerNews | SourceRoute::Reddit => "providers-v1",
        _ => "html-v1",
    }
}

async fn enqueue_queued_report(
    transaction: &mut PgTransaction<'_>,
    command: &CaptureCommand,
) -> Result<(), ConsumeError> {
    let event_id = EventId::new_v7();
    let mut envelope = EventEnvelope {
        event_id,
        event_type: OperationReported::event_type(),
        occurred_at: WireTimestamp::now(),
        producer: ProducerName::parse(PRODUCER)?,
        aggregate_id: command.operation_id.as_entity_ref(),
        correlation_id: command.correlation_id.clone(),
        causation_id: Some(EntityRef::parse(&format!(
            "command:{}",
            command.command_id
        ))?),
        tenant_id: Some(command.tenant_id),
        schema_version: EnvelopeSchemaVersion::CURRENT,
        payload: serde_json::Map::new(),
        extensions: Extensions::new(),
    };
    envelope.set_payload(&OperationReported {
        operation_id: command.operation_id,
        status: OperationStatus::Queued,
        stage: None,
        progress_percent: None,
        results: Vec::new(),
        error: None,
        warnings: Vec::new(),
        extensions: Extensions::new(),
    })?;
    let serialized = serde_json::to_value(&envelope)?;

    sqlx::query(
        "insert into extractor.outbox_events
             (outbox_id, message_id, causation_command_id, operation_id, subject, payload,
              enqueued_at, next_attempt_at)
         values ($1, $2, $3, $4, $5, $6, transaction_timestamp(), transaction_timestamp())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(event_id.0)
    .bind(command.command_id)
    .bind(command.operation_id.0)
    .bind(REPORT_SUBJECT)
    .bind(serialized)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn routing_policy() -> RoutingPolicy {
    RoutingPolicy {
        max_url_length: 8_192,
        allowed_ports: vec![80, 443],
    }
}

const fn route_name(route: SourceRoute) -> &'static str {
    match route {
        SourceRoute::GitHub => "github",
        SourceRoute::X => "x",
        SourceRoute::Instagram => "instagram",
        SourceRoute::Threads => "threads",
        SourceRoute::Reddit => "reddit",
        SourceRoute::HackerNews => "hacker_news",
        SourceRoute::YouTube => "youtube",
        SourceRoute::Pdf => "pdf",
        SourceRoute::GenericWeb => "generic_web",
    }
}
