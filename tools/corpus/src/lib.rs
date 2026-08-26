#![forbid(unsafe_code)]

//! Offline golden-corpus verification for Ratatoskr Extractor.

use std::fs;
use std::path::{Path, PathBuf};

use extractor_document_ir::{HtmlDocumentInput, ParseLimits, from_html};
use extractor_pdf::{PdfDocumentInput, PdfParseLimits, from_pdf};
use extractor_providers::{ProviderInput, ProviderLimits, SourceRoute, from_provider};
use extractor_youtube::{PlayerMeta, Segment, TranscriptInput, YoutubeLimits, from_transcript};
use ratatoskr_document_contracts::{Document, DocumentAddress};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, DocumentId, MediaType,
    canonical_json,
};
use serde::Deserialize;

pub mod performance;

const HTML_SEMANTIC: &[u8] =
    include_bytes!("../../../crates/document-ir/tests/fixtures/semantic.html");
const PDF_TEXT: &[u8] = include_bytes!("../../../crates/pdf/tests/fixtures/text-two-pages.pdf");
const HACKER_NEWS: &[u8] = include_bytes!("../../../crates/providers/tests/fixtures/hn-story.json");
const REDDIT: &[u8] = include_bytes!("../../../crates/providers/tests/fixtures/reddit-post.json");
const TRANSCRIPT: &[u8] = include_bytes!("../fixtures/transcripts/english.json");

/// Names of every committed source-to-Document-IR corpus case.
pub const CASE_NAMES: [&str; 5] = [
    "html-semantic",
    "pdf-direct",
    "hacker-news",
    "reddit",
    "youtube-transcript",
];

/// Why corpus verification could not complete.
#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    /// The requested case does not exist in the committed corpus manifest.
    #[error("unknown corpus case: {0}")]
    UnknownCase(String),
    /// Source or expected data could not be read from the committed corpus.
    #[error("corpus case {case} could not read {path}: {source}")]
    Read {
        /// Corpus case being processed.
        case: String,
        /// Source or expected file path.
        path: PathBuf,
        /// Underlying filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A source document could not become Document IR.
    #[error("corpus case {case} could not produce Document IR: {message}")]
    Extraction {
        /// Corpus case being processed.
        case: String,
        /// Stable rendered error text from the owned adapter.
        message: String,
    },
    /// An expected Document IR file is not valid JSON.
    #[error("corpus case {case} has invalid expected JSON: {source}")]
    ExpectedJson {
        /// Corpus case being processed.
        case: String,
        /// JSON decoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// A produced Document IR could not be serialized.
    #[error("corpus case {case} could not serialize Document IR: {source}")]
    ActualJson {
        /// Corpus case being processed.
        case: String,
        /// JSON encoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// The committed expectation differs from the current normalized output.
    #[error(
        "corpus case {case} differs from its expected Document IR\nexpected: {expected}\nactual: {actual}"
    )]
    Mismatch {
        /// Corpus case being processed.
        case: String,
        /// Pretty canonical expected JSON.
        expected: String,
        /// Pretty canonical actual JSON.
        actual: String,
    },
}

/// Verifies every committed corpus case.
///
/// # Errors
///
/// Returns [`CorpusError`] when verification cannot complete.
pub fn verify() -> Result<(), CorpusError> {
    verify_at(corpus_root())
}

/// Verifies every committed corpus case rooted at `root` without writing files.
///
/// # Errors
///
/// Returns [`CorpusError`] for an absent, invalid, or mismatched expectation.
pub fn verify_at(root: impl AsRef<Path>) -> Result<(), CorpusError> {
    let root = root.as_ref();
    for case in CASE_NAMES {
        verify_case_at(root, case)?;
    }
    Ok(())
}

/// Verifies one named corpus case without writing files.
///
/// # Errors
///
/// Returns [`CorpusError`] for an unknown case or mismatched expectation.
pub fn verify_case_at(root: impl AsRef<Path>, case: &str) -> Result<(), CorpusError> {
    let root = root.as_ref();
    let actual = document_for_case(case)?;
    let actual = pretty_document(case, &actual)?;
    let expected_path = expected_path(root, case)?;
    let expected = fs::read(&expected_path).map_err(|source| CorpusError::Read {
        case: case.to_owned(),
        path: expected_path.clone(),
        source,
    })?;
    let expected_value =
        serde_json::from_slice::<serde_json::Value>(&expected).map_err(|source| {
            CorpusError::ExpectedJson {
                case: case.to_owned(),
                source,
            }
        })?;
    let actual_value = serde_json::from_str::<serde_json::Value>(&actual).map_err(|source| {
        CorpusError::ActualJson {
            case: case.to_owned(),
            source,
        }
    })?;
    if expected_value == actual_value {
        Ok(())
    } else {
        Err(CorpusError::Mismatch {
            case: case.to_owned(),
            expected: serde_json::to_string_pretty(&expected_value).map_err(|source| {
                CorpusError::ActualJson {
                    case: case.to_owned(),
                    source,
                }
            })?,
            actual,
        })
    }
}

/// Regenerates the expected JSON for exactly one named corpus case.
///
/// # Errors
///
/// Returns [`CorpusError`] for an unknown case, extraction failure, or filesystem failure.
pub fn bless_case_at(root: impl AsRef<Path>, case: &str) -> Result<PathBuf, CorpusError> {
    let root = root.as_ref();
    let document = document_for_case(case)?;
    let expected = pretty_document(case, &document)?;
    let path = expected_path(root, case)?;
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| CorpusError::UnknownCase(case.to_owned()))?,
    )
    .map_err(|source| CorpusError::Read {
        case: case.to_owned(),
        path: path.clone(),
        source,
    })?;
    fs::write(&path, expected).map_err(|source| CorpusError::Read {
        case: case.to_owned(),
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Returns the repository-owned corpus directory.
#[must_use]
pub fn corpus_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn expected_path(root: &Path, case: &str) -> Result<PathBuf, CorpusError> {
    if CASE_NAMES.contains(&case) {
        Ok(root.join("expected").join(format!("{case}.json")))
    } else {
        Err(CorpusError::UnknownCase(case.to_owned()))
    }
}

fn pretty_document(case: &str, document: &Document) -> Result<String, CorpusError> {
    canonical_json(document).map_err(|source| CorpusError::ActualJson {
        case: case.to_owned(),
        source,
    })
}

fn document_for_case(case: &str) -> Result<Document, CorpusError> {
    match case {
        "html-semantic" => html_document(),
        "pdf-direct" => pdf_document(),
        "hacker-news" => provider_document(SourceRoute::HackerNews, HACKER_NEWS, case),
        "reddit" => provider_document(SourceRoute::Reddit, REDDIT, case),
        "youtube-transcript" => transcript_document(),
        _ => Err(CorpusError::UnknownCase(case.to_owned())),
    }
}

fn html_document() -> Result<Document, CorpusError> {
    from_html(
        HtmlDocumentInput {
            document_id: document_id("000000000101")?,
            source_address: address("https://corpus.example/html-semantic")?,
            source_blob: blob(HTML_SEMANTIC.len(), "text/html")?,
            bytes: HTML_SEMANTIC,
        },
        ParseLimits {
            max_input_bytes: 65_536,
            max_dom_nodes: 2_000,
        },
    )
    .map(|extraction| extraction.document)
    .map_err(|error| extraction_error("html-semantic", error))
}

fn pdf_document() -> Result<Document, CorpusError> {
    from_pdf(
        PdfDocumentInput {
            document_id: document_id("000000000102")?,
            source_address: address("https://corpus.example/direct.pdf")?,
            source_blob: blob(PDF_TEXT.len(), "application/pdf")?,
            bytes: PDF_TEXT,
        },
        PdfParseLimits {
            max_input_bytes: 65_536,
            max_pages: 16,
            max_text_bytes: 1_048_576,
        },
    )
    .map(|extraction| extraction.document)
    .map_err(|error| extraction_error("pdf-direct", error))
}

fn provider_document(
    route: SourceRoute,
    bytes: &[u8],
    case: &str,
) -> Result<Document, CorpusError> {
    from_provider(
        ProviderInput {
            document_id: document_id(if matches!(route, SourceRoute::HackerNews) {
                "000000000103"
            } else {
                "000000000104"
            })?,
            source_address: address(if matches!(route, SourceRoute::HackerNews) {
                "https://hn.algolia.com/api/v1/items/424242"
            } else {
                "https://www.reddit.com/r/corpus/comments/abc123/post/.json"
            })?,
            source_blob: blob(bytes.len(), "application/json")?,
            route,
            bytes,
        },
        ProviderLimits {
            max_input_bytes: 65_536,
            max_blocks: 2_000,
        },
    )
    .map(|extraction| extraction.document)
    .map_err(|error| extraction_error(case, error))
}

fn transcript_document() -> Result<Document, CorpusError> {
    let fixture = serde_json::from_slice::<TranscriptFixture>(TRANSCRIPT).map_err(|source| {
        CorpusError::ExpectedJson {
            case: "youtube-transcript".to_owned(),
            source,
        }
    })?;
    let meta = PlayerMeta {
        title: Some(fixture.title),
        channel: Some(fixture.channel),
        duration_seconds: Some(fixture.duration_seconds),
        tracks: Vec::new(),
    };
    let segments = fixture
        .segments
        .into_iter()
        .map(|segment| Segment {
            start_ms: segment.start_ms,
            duration_ms: segment.duration_ms,
            text: segment.text,
        })
        .collect();
    from_transcript(
        TranscriptInput {
            document_id: document_id("000000000105")?,
            source_address: address("https://www.youtube.com/watch?v=dQw4w9WgXcQ")?,
            source_blob: blob(TRANSCRIPT.len(), "application/json")?,
            meta: &meta,
            segments,
            language: "en",
        },
        YoutubeLimits {
            max_page_bytes: 65_536,
            max_track_bytes: 65_536,
            max_blocks: 32,
            max_segments: 1_000,
            max_segment_characters: 1_000,
        },
    )
    .map(|extraction| extraction.document)
    .map_err(|error| extraction_error("youtube-transcript", error))
}

fn document_id(suffix: &str) -> Result<DocumentId, CorpusError> {
    DocumentId::parse(&format!("018f0000-0000-7000-8000-{suffix}"))
        .map_err(|error| extraction_error("identity", error))
}

fn address(value: &str) -> Result<DocumentAddress, CorpusError> {
    DocumentAddress::parse(value).map_err(|error| extraction_error("identity", error))
}

fn blob(length: usize, media_type: &str) -> Result<BlobRef, CorpusError> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")
            .map_err(|error| extraction_error("identity", error))?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"ab".repeat(32))
                .map_err(|error| extraction_error("identity", error))?,
        },
        media_type: MediaType::parse(media_type)
            .map_err(|error| extraction_error("identity", error))?,
        length_bytes: u64::try_from(length).map_err(|error| extraction_error("identity", error))?,
    })
}

fn extraction_error(case: &str, error: impl std::fmt::Display) -> CorpusError {
    CorpusError::Extraction {
        case: case.to_owned(),
        message: error.to_string(),
    }
}

#[derive(Debug, Deserialize)]
struct TranscriptFixture {
    title: String,
    channel: String,
    duration_seconds: u64,
    segments: Vec<TranscriptSegmentFixture>,
}

#[derive(Debug, Deserialize)]
struct TranscriptSegmentFixture {
    start_ms: u64,
    duration_ms: u64,
    text: String,
}
