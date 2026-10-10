//! Typed capture-command intake: decode, inbox claim, source and run persistence.

use extractor_url_routing::{RoutingPolicy, SourceRoute, classify, normalize};
use ratatoskr_document_contracts::ContentCaptureRequested;
use ratatoskr_event_envelope::{
    CommandEnvelope, EnvelopeSchemaVersion, EventEnvelope, EventPayload as _, ProducerName,
};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, EntityRef, EventId,
    Extensions, MediaType, OperationId, TenantRef, WireTimestamp,
};
use ratatoskr_operation_contracts::{OperationReported, OperationStatus};
use sqlx::{PgPool, PgTransaction};

use crate::{
    CAPTURE_COMMAND_TYPE, CAPTURE_SUBJECT, ConsumeError, PRODUCER, QueuedRun, REPORT_SUBJECT,
    Reception,
};

/// Deployables allowed to request captures (XR-021 CONTRACTS.md S01).
const ALLOWED_PRODUCERS: [&str; 2] = ["ratatoskr-platform", "ratatoskr-x"];

/// What a capture command asks the extractor to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureSource {
    /// An untrusted public address, syntactically parsed only.
    Url(url::Url),
    /// Bytes another service stored, named by a content-addressed reference.
    Blob(BlobRef),
}

/// Validated request to capture one public address or one stored blob.
#[derive(Debug, Clone)]
pub struct CaptureCommand {
    /// At-least-once deduplication key.
    pub command_id: uuid::Uuid,
    /// Platform operation whose work is requested.
    pub operation_id: OperationId,
    /// Deployable that issued the command.
    pub producer: ProducerName,
    /// Owner of the requested content.
    pub tenant_id: TenantRef,
    /// Cross-process correlation reference.
    pub correlation_id: EntityRef,
    /// Lowercase hexadecimal SHA-256 of the caller's idempotency string.
    pub idempotency_key: String,
    /// When the producer issued the command.
    pub requested_at: WireTimestamp,
    /// What to read.
    pub source: CaptureSource,
}

/// Decodes the typed `content.capture.requested.v1` command envelope.
///
/// Unknown additive members are ignored. URL destination policy remains the safe fetcher's
/// responsibility.
///
/// # Errors
///
/// Returns [`ConsumeError`] when the subject, command type, tenant, aggregate or typed payload is
/// invalid.
pub fn decode_capture(subject: &str, payload: &[u8]) -> Result<CaptureCommand, ConsumeError> {
    if subject != CAPTURE_SUBJECT {
        return Err(ConsumeError::InvalidSubject);
    }
    let envelope = CommandEnvelope::from_json(payload)?;
    if envelope.command_type.to_wire() != CAPTURE_COMMAND_TYPE {
        return Err(ConsumeError::InvalidCommandType);
    }
    if !ALLOWED_PRODUCERS.contains(&envelope.producer.as_str()) {
        return Err(ConsumeError::ForeignProducer);
    }
    let tenant_id = envelope.tenant_id.ok_or(ConsumeError::MissingTenant)?;
    let request = envelope.payload_as::<ContentCaptureRequested>()?;
    if envelope.aggregate_id != request.operation_id.as_entity_ref() {
        return Err(ConsumeError::AggregateMismatch);
    }
    let source = match (request.url, request.blob) {
        (Some(address), None) => {
            let url =
                url::Url::parse(address.as_str()).map_err(|_| ConsumeError::InvalidUrlScheme)?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(ConsumeError::InvalidUrlScheme);
            }
            CaptureSource::Url(url)
        }
        (None, Some(blob)) => {
            if !matches!(
                blob.digest.algorithm,
                ratatoskr_identifiers::DigestAlgorithm::Sha256
            ) {
                return Err(ConsumeError::InvalidArtifact);
            }
            CaptureSource::Blob(blob)
        }
        _ => return Err(ConsumeError::InvalidPayload),
    };
    Ok(CaptureCommand {
        command_id: envelope.command_id.0,
        operation_id: request.operation_id,
        tenant_id,
        producer: envelope.producer,
        correlation_id: envelope.correlation_id,
        idempotency_key: request.idempotency_key.hex.as_str().to_owned(),
        requested_at: envelope.issued_at,
        source,
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
    .bind(command.producer.as_str())
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
    let claimed = sqlx::query_as::<_, ClaimedRow>(
        "with next as (
             select r.run_id, r.document_id, s.normalized_url, s.classification,
                    s.blob_owner, s.blob_digest_hex, s.blob_media_type, s.blob_length_bytes
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
          returning r.run_id, next.document_id, next.normalized_url, next.classification,
                    next.blob_owner, next.blob_digest_hex, next.blob_media_type,
                    next.blob_length_bytes",
    )
    .bind(claimed_by)
    .bind(lease_seconds.clamp(1, 3_600))
    .fetch_optional(pool)
    .await?;
    claimed.map(queued_run).transpose()
}

/// One claimed row: run, document, source address, classification and the optional blob columns.
type ClaimedRow = (
    uuid::Uuid,
    uuid::Uuid,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

fn queued_run(row: ClaimedRow) -> Result<QueuedRun, ConsumeError> {
    let (run_id, document_id, url, classification, owner, digest, media_type, length) = row;
    let blob = match (owner, digest, media_type, length) {
        (Some(owner), Some(digest), Some(media_type), Some(length)) => Some(BlobRef {
            owner_service: BlobOwner::parse(&owner).map_err(|_| ConsumeError::InvalidArtifact)?,
            digest: ContentDigest {
                algorithm: DigestAlgorithm::Sha256,
                hex: DigestHex::parse(&digest).map_err(|_| ConsumeError::InvalidArtifact)?,
            },
            media_type: MediaType::parse(&media_type).map_err(|_| ConsumeError::InvalidArtifact)?,
            length_bytes: u64::try_from(length).map_err(|_| ConsumeError::InvalidArtifact)?,
        }),
        _ => None,
    };
    Ok(QueuedRun {
        run_id,
        document_id: DocumentId(document_id),
        url,
        classification,
        blob,
    })
}

/// The source row a run reads and the pipeline generations recorded with it.
struct IntakeSource {
    source_id: uuid::Uuid,
    policy_version: &'static str,
    normalizer_version: &'static str,
    parser_version: &'static str,
}

async fn queue_run(
    transaction: &mut PgTransaction<'_>,
    command: &CaptureCommand,
) -> Result<(), ConsumeError> {
    let source = match &command.source {
        CaptureSource::Url(url) => url_source(transaction, command, url).await?,
        CaptureSource::Blob(blob) => blob_source(transaction, command, blob).await?,
    };
    sqlx::query(
        "insert into extractor.extraction_runs
             (run_id, command_id, operation_id, owner_id, correlation_id, source_id, document_id,
              status, policy_version, normalizer_version, parser_version, queued_at)
         values ($1, $2, $3, $4, $5, $6, $7, 'queued', $8, $9, $10, transaction_timestamp())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(command.command_id)
    .bind(command.operation_id.0)
    .bind(command.tenant_id.user_id().0)
    .bind(command.correlation_id.to_string())
    .bind(source.source_id)
    .bind(DocumentId::new_v7().0)
    .bind(source.policy_version)
    .bind(source.normalizer_version)
    .bind(source.parser_version)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn url_source(
    transaction: &mut PgTransaction<'_>,
    command: &CaptureCommand,
    url: &url::Url,
) -> Result<IntakeSource, ConsumeError> {
    let normalized = normalize(url.as_str(), &routing_policy())?;
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
    Ok(IntakeSource {
        source_id,
        policy_version: "ssrf-v1",
        normalizer_version: "url-v1",
        parser_version: parser_version(route),
    })
}

/// Records a peer-owned blob as a source. The address is the content-addressed URN, so the same
/// owner re-sending the same bytes reuses one source row; the URL normalizer and the SSRF policy
/// do not apply because nothing is fetched.
async fn blob_source(
    transaction: &mut PgTransaction<'_>,
    command: &CaptureCommand,
    blob: &BlobRef,
) -> Result<IntakeSource, ConsumeError> {
    let length = i64::try_from(blob.length_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
    let address = format!("urn:ratatoskr:blob:sha256:{}", blob.digest.hex.as_str());
    let source_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "insert into extractor.sources
             (source_id, owner_id, original_url, normalized_url, canonical_url, host,
              classification, created_at, source_kind, blob_owner, blob_digest_hex,
              blob_media_type, blob_length_bytes)
         values ($1, $2, $3, $3, $3, $4, 'blob', transaction_timestamp(), 'blob', $4, $5, $6, $7)
         on conflict (owner_id, normalized_url) do update
             set canonical_url = excluded.canonical_url
         returning source_id",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(command.tenant_id.user_id().0)
    .bind(address)
    .bind(blob.owner_service.as_str())
    .bind(blob.digest.hex.as_str())
    .bind(blob.media_type.as_str())
    .bind(length)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(IntakeSource {
        source_id,
        policy_version: "peer-blob-v1",
        normalizer_version: "blob-v1",
        parser_version: "pdf-v1",
    })
}

/// Names the parser generation expected for a classified source at intake time.
const fn parser_version(route: SourceRoute) -> &'static str {
    match route {
        SourceRoute::Pdf => "pdf-v1",
        SourceRoute::YouTube => "youtube-v1",
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
