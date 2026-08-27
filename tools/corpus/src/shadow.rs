//! Offline comparison of committed legacy observations with current Document IR.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use ratatoskr_document_contracts::{Document, DocumentBlock};
use serde::Deserialize;

use crate::{CorpusError, corpus_root, document_for_case};

const CASES_PATH: &str = "shadow/cases.json";
const REPORT_PATH: &str = "shadow/report.md";

/// Renders the report for the committed offline shadow cases.
///
/// # Errors
///
/// Returns [`ShadowError`] when committed comparison inputs cannot be read or a current corpus
/// conversion fails.
pub fn render_report() -> Result<String, ShadowError> {
    render_report_at(corpus_root())
}

/// Renders the report for offline shadow cases rooted at `root`.
///
/// # Errors
///
/// Returns [`ShadowError`] when comparison inputs cannot be read or current conversion fails.
pub fn render_report_at(root: impl AsRef<Path>) -> Result<String, ShadowError> {
    let root = root.as_ref();
    let cases_path = root.join(CASES_PATH);
    let source = fs::read(&cases_path).map_err(|source| ShadowError::Read {
        path: cases_path.clone(),
        source,
    })?;
    let fixture = serde_json::from_slice::<ShadowFixture>(&source).map_err(|source| {
        ShadowError::InvalidFixture {
            path: cases_path,
            source,
        }
    })?;
    Report::from_fixture(&fixture)?.render()
}

/// Verifies the review artifact rooted at `root`.
///
/// # Errors
///
/// Returns an error when the rendered report differs from the committed artifact.
pub fn verify_report_at(root: impl AsRef<Path>) -> Result<(), ShadowError> {
    let root = root.as_ref();
    let actual = render_report_at(root)?;
    let path = root.join(REPORT_PATH);
    let expected = fs::read_to_string(&path).map_err(|source| ShadowError::Read {
        path: path.clone(),
        source,
    })?;
    if expected == actual {
        Ok(())
    } else {
        Err(ShadowError::Mismatch { expected, actual })
    }
}

/// Why a shadow comparison report could not be rendered.
#[derive(Debug, thiserror::Error)]
pub enum ShadowError {
    /// A committed fixture could not be read.
    #[error("could not read shadow comparison fixture {path}: {source}")]
    Read {
        /// Fixture path.
        path: std::path::PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A committed fixture could not be decoded.
    #[error("could not decode shadow comparison fixture {path}: {source}")]
    InvalidFixture {
        /// Fixture path.
        path: std::path::PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// A source class lacks reviewable criteria.
    #[error("shadow comparison case {case} has no criteria for {source_class}")]
    MissingCriteria {
        /// Case name.
        case: String,
        /// Source class missing criteria.
        source_class: String,
    },
    /// The current deterministic corpus conversion failed.
    #[error("shadow comparison case {case} could not produce current Document IR: {source}")]
    CurrentExtraction {
        /// Case name.
        case: String,
        /// Current corpus failure.
        #[source]
        source: CorpusError,
    },
    /// Report rendering could not write to its in-memory buffer.
    #[error("could not render shadow comparison report")]
    Formatting,
    /// A fixture contains more values than a finite report metric can represent.
    #[error("shadow comparison {metric} exceeds the supported report size")]
    MetricSize {
        /// Metric whose count overflowed the report representation.
        metric: &'static str,
    },
    /// The generated report differs from the committed review artifact.
    #[error("shadow comparison report differs from its committed review artifact")]
    Mismatch {
        /// Committed report content.
        expected: String,
        /// Generated report content.
        actual: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SourceClass {
    WebArticle,
    YoutubeTranscript,
    XPost,
}

impl SourceClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::WebArticle => "web_article",
            Self::YoutubeTranscript => "youtube_transcript",
            Self::XPost => "x_post",
        }
    }
}

impl std::fmt::Display for SourceClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShadowFixture {
    legacy_archive: LegacyArchive,
    criteria: BTreeMap<SourceClass, ClassCriteria>,
    cases: Vec<ShadowCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyArchive {
    repository: String,
    revision: String,
    read_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassCriteria {
    minimum_overlap: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShadowCase {
    name: String,
    source_class: SourceClass,
    source_address: String,
    current_case: Option<String>,
    legacy: LegacyObservation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyObservation {
    capture_path: String,
    success: bool,
    text: String,
    block_kinds: BTreeMap<String, usize>,
}

#[derive(Debug)]
struct Report {
    archive: LegacyArchive,
    classes: BTreeMap<SourceClass, ClassReport>,
}

impl Report {
    fn from_fixture(fixture: &ShadowFixture) -> Result<Self, ShadowError> {
        let mut classes = BTreeMap::new();
        for case in &fixture.cases {
            let criteria = fixture.criteria.get(&case.source_class).ok_or_else(|| {
                ShadowError::MissingCriteria {
                    case: case.name.clone(),
                    source_class: case.source_class.to_string(),
                }
            })?;
            let entry = classes
                .entry(case.source_class)
                .or_insert_with(|| ClassReport::new(criteria.minimum_overlap));
            entry.cases.push(CaseReport::from_case(case)?);
        }
        for (source_class, criteria) in &fixture.criteria {
            classes
                .entry(*source_class)
                .or_insert_with(|| ClassReport::new(criteria.minimum_overlap));
        }
        Ok(Self {
            archive: LegacyArchive {
                repository: fixture.legacy_archive.repository.clone(),
                revision: fixture.legacy_archive.revision.clone(),
                read_only: fixture.legacy_archive.read_only,
            },
            classes,
        })
    }

    fn render(&self) -> Result<String, ShadowError> {
        let mut output = String::new();
        line(&mut output, "# Legacy shadow comparison report")?;
        line(&mut output, "")?;
        line(
            &mut output,
            &format!(
                "Legacy archive: `{}` at `{}` (read-only: {}).",
                self.archive.repository, self.archive.revision, self.archive.read_only
            ),
        )?;
        line(
            &mut output,
            "This is an offline measurement only. Owner approval and a separate cutover change are required before any traffic switch.",
        )?;
        for (source_class, class) in &self.classes {
            class.render(&mut output, *source_class)?;
        }
        Ok(output)
    }
}

#[derive(Debug)]
struct ClassReport {
    minimum_overlap: f64,
    cases: Vec<CaseReport>,
}

impl ClassReport {
    fn new(minimum_overlap: f64) -> Self {
        Self {
            minimum_overlap,
            cases: Vec::new(),
        }
    }

    fn render(&self, output: &mut String, source_class: SourceClass) -> Result<(), ShadowError> {
        let verdict = self.verdict();
        line(output, "")?;
        line(output, &format!("## {source_class}: {}", verdict.as_str()))?;
        line(output, "")?;
        line(output, &format!("- samples: {}", self.cases.len()))?;
        line(
            output,
            &format!(
                "- success rate: legacy {}/{} ({:.1}%), current {}/{} ({:.1}%)",
                self.legacy_successes(),
                self.cases.len(),
                percent(self.legacy_successes(), self.cases.len())?,
                self.current_successes(),
                self.cases.len(),
                percent(self.current_successes(), self.cases.len())?
            ),
        )?;
        line(
            output,
            &format!("- minimum content overlap: {:.3}", self.minimum_overlap),
        )?;
        line(output, "- legacy block statistics:")?;
        render_statistics(output, self.legacy_blocks())?;
        line(output, "- current IR block statistics:")?;
        render_statistics(output, self.current_blocks())?;
        line(output, &format!("- verdict: {}", self.verdict_reason()))?;
        line(output, "")?;
        line(output, "### Cases")?;
        for case in &self.cases {
            line(output, &case.render())?;
        }
        Ok(())
    }

    fn verdict(&self) -> Verdict {
        if self.legacy_successes() == 0 {
            return Verdict::InsufficientEvidence;
        }
        if self.current_successes() < self.legacy_successes()
            || self
                .cases
                .iter()
                .any(|case| case.legacy.success && !case.current.is_success())
            || self.cases.iter().any(|case| {
                case.overlap
                    .is_some_and(|overlap| overlap < self.minimum_overlap)
            })
        {
            Verdict::Hold
        } else {
            Verdict::Approve
        }
    }

    fn verdict_reason(&self) -> String {
        match self.verdict() {
            Verdict::Approve => "all committed criteria pass".to_owned(),
            Verdict::InsufficientEvidence => "no legacy-success sample is committed".to_owned(),
            Verdict::Hold => self
                .cases
                .iter()
                .find_map(|case| case.hold_reason(self.minimum_overlap))
                .unwrap_or_else(|| "current success rate is below legacy".to_owned()),
        }
    }

    fn legacy_successes(&self) -> usize {
        self.cases.iter().filter(|case| case.legacy.success).count()
    }

    fn current_successes(&self) -> usize {
        self.cases
            .iter()
            .filter(|case| case.current.is_success())
            .count()
    }

    fn legacy_blocks(&self) -> BTreeMap<String, usize> {
        aggregate_blocks(self.cases.iter().map(|case| &case.legacy.block_kinds))
    }

    fn current_blocks(&self) -> BTreeMap<String, usize> {
        aggregate_blocks(self.cases.iter().filter_map(CaseReport::current_blocks))
    }
}

#[derive(Debug)]
struct CaseReport {
    name: String,
    source_address: String,
    legacy: LegacyObservation,
    current: CurrentOutcome,
    overlap: Option<f64>,
}

impl CaseReport {
    fn from_case(case: &ShadowCase) -> Result<Self, ShadowError> {
        let current = match &case.current_case {
            Some(current_case) => CurrentOutcome::Success(CurrentDocument::from_document(
                document_for_case(current_case).map_err(|source| {
                    ShadowError::CurrentExtraction {
                        case: case.name.clone(),
                        source,
                    }
                })?,
            )),
            None => CurrentOutcome::Unsupported,
        };
        let overlap = match &current {
            CurrentOutcome::Success(document) if case.legacy.success => {
                Some(content_overlap(&case.legacy.text, &document.text)?)
            }
            CurrentOutcome::Unsupported | CurrentOutcome::Success(_) => None,
        };
        Ok(Self {
            name: case.name.clone(),
            source_address: case.source_address.clone(),
            legacy: LegacyObservation {
                capture_path: case.legacy.capture_path.clone(),
                success: case.legacy.success,
                text: case.legacy.text.clone(),
                block_kinds: case.legacy.block_kinds.clone(),
            },
            current,
            overlap,
        })
    }

    fn render(&self) -> String {
        let legacy = if self.legacy.success {
            "success"
        } else {
            "failure"
        };
        let current = match self.current {
            CurrentOutcome::Success(_) => "success".to_owned(),
            CurrentOutcome::Unsupported => "unsupported".to_owned(),
        };
        let overlap = self
            .overlap
            .map(|overlap| format!(", overlap {overlap:.3}"))
            .unwrap_or_default();
        format!(
            "- `{}` ({}) — legacy {legacy} via `{}`, current {current}{overlap}",
            self.name, self.source_address, self.legacy.capture_path
        )
    }

    fn hold_reason(&self, minimum_overlap: f64) -> Option<String> {
        match self.current {
            CurrentOutcome::Unsupported if self.legacy.success => {
                Some(format!("current extraction is unsupported: {}", self.name))
            }
            CurrentOutcome::Success(_)
                if self.legacy.success
                    && self.overlap.is_some_and(|value| value < minimum_overlap) =>
            {
                Some(format!(
                    "content overlap below {:.3}: {} ({:.3})",
                    minimum_overlap,
                    self.name,
                    self.overlap.unwrap_or_default()
                ))
            }
            _ => None,
        }
    }

    fn current_blocks(&self) -> Option<&BTreeMap<String, usize>> {
        match &self.current {
            CurrentOutcome::Success(document) => Some(&document.block_kinds),
            CurrentOutcome::Unsupported => None,
        }
    }
}

#[derive(Debug)]
enum CurrentOutcome {
    Success(CurrentDocument),
    Unsupported,
}

impl CurrentOutcome {
    fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }
}

#[derive(Debug)]
struct CurrentDocument {
    text: String,
    block_kinds: BTreeMap<String, usize>,
}

impl CurrentDocument {
    fn from_document(document: Document) -> Self {
        let mut text = Vec::new();
        let mut block_kinds = BTreeMap::new();
        for block in document.blocks {
            let (kind, value) = match block {
                DocumentBlock::Heading { text, .. } => ("heading", text),
                DocumentBlock::Paragraph { text, .. } => ("paragraph", text),
                _ => continue,
            };
            text.push(value);
            *block_kinds.entry(kind.to_owned()).or_default() += 1;
        }
        Self {
            text: text.join(" "),
            block_kinds,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Verdict {
    Approve,
    Hold,
    InsufficientEvidence,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Hold => "hold",
            Self::InsufficientEvidence => "insufficient-evidence",
        }
    }
}

fn content_overlap(legacy: &str, current: &str) -> Result<f64, ShadowError> {
    let legacy = tokens(legacy);
    if legacy.is_empty() {
        return Ok(1.0);
    }
    let current = tokens(current);
    let covered = legacy.intersection(&current).count();
    Ok(count_as_f64(covered, "content-overlap tokens")?
        / count_as_f64(legacy.len(), "content-overlap tokens")?)
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn aggregate_blocks<'a>(
    statistics: impl Iterator<Item = &'a BTreeMap<String, usize>>,
) -> BTreeMap<String, usize> {
    let mut total = BTreeMap::new();
    for blocks in statistics {
        for (kind, count) in blocks {
            *total.entry(kind.clone()).or_default() += count;
        }
    }
    total
}

fn render_statistics(
    output: &mut String,
    statistics: BTreeMap<String, usize>,
) -> Result<(), ShadowError> {
    if statistics.is_empty() {
        line(output, "  - none")?;
    } else {
        for (kind, count) in statistics {
            line(output, &format!("  - {kind}: {count}"))?;
        }
    }
    Ok(())
}

fn percent(numerator: usize, denominator: usize) -> Result<f64, ShadowError> {
    if denominator == 0 {
        Ok(0.0)
    } else {
        Ok(count_as_f64(numerator, "success-rate samples")? * 100.0
            / count_as_f64(denominator, "success-rate samples")?)
    }
}

fn count_as_f64(value: usize, metric: &'static str) -> Result<f64, ShadowError> {
    u32::try_from(value)
        .map(f64::from)
        .map_err(|_| ShadowError::MetricSize { metric })
}

fn line(output: &mut String, value: &str) -> Result<(), ShadowError> {
    writeln!(output, "{value}").map_err(|_| ShadowError::Formatting)
}
