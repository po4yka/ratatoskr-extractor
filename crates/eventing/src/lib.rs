#![forbid(unsafe_code)]
//! Extractor command inbox and transactional report outbox.

mod capture;
mod completion;
mod consumer;
mod outbox;
mod render;
mod terminal;
mod topology;

/// Shared fleet event stream carrying every `evt.*` subject.
pub const EVENTS_STREAM: &str = "ratatoskr_events";
/// Shared fleet command stream carrying every `cmd.*` subject.
pub const COMMAND_STREAM: &str = "ratatoskr_commands";

pub use capture::{
    CaptureCommand, CaptureSource, claim_queued_run, consume_capture, decode_capture,
};
pub use completion::{
    complete_blob_document, complete_document, fail_run, record_fetch, reject_blob_quality,
    reject_quality, store_document_ir,
};
pub use consumer::{ConsumerReport, run_command_consumer};
pub use outbox::{
    NatsPublisher, OutboxReport, PublishError, Publisher, ensure_event_stream_on, run_outbox_once,
};
pub use render::{
    RenderBudget, RenderBus, RenderOutcome, RenderRequestError, consume_render_budget,
    request_render,
};
pub use terminal::ResolutionStep;
pub use topology::{CAPTURE_DURABLE, TopologyError, verify_bus_topology};

use extractor_blob_store::BlobStoreError;
use ratatoskr_event_envelope::EnvelopeError;
use ratatoskr_identifiers::{BlobRef, DocumentId};

const CAPTURE_COMMAND_TYPE: &str = "content.capture.requested.v1";
const CAPTURE_SUBJECT: &str = "cmd.content.capture.requested.v1";
const REPORT_SUBJECT: &str = "evt.platform.operation.reported.v1";
const PRODUCER: &str = "ratatoskr-extractor";

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
    /// Peer-owned bytes this run reads, present exactly for a blob capture.
    pub blob: Option<BlobRef>,
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
    /// The command is not valid JSON for the published envelope or payload shape.
    #[error("the capture command is malformed")]
    Command(#[from] serde_json::Error),
    /// The command envelope or its typed payload violates the contract.
    #[error("the capture command envelope is invalid")]
    CommandEnvelope(#[from] ratatoskr_event_envelope::CommandError),
    /// The envelope producer is not allowed to request captures.
    #[error("the capture command producer is not allowed")]
    ForeignProducer,
    /// The envelope has no owner, which every capture requires.
    #[error("the capture command has no tenant")]
    MissingTenant,
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
    /// The envelope aggregate does not name the payload operation.
    #[error("the command aggregate does not match the payload operation")]
    AggregateMismatch,
    /// A typed payload unexpectedly did not serialize as an object.
    #[error("the event payload is not an object")]
    InvalidPayload,
    /// The local content-addressed store refused the canonical IR bytes.
    #[error("the document artifact could not be stored")]
    ArtifactStore(#[from] BlobStoreError),
}
