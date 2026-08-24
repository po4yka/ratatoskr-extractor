use extractor_document_ir::{CandidateDecision, QualityReason};
use sqlx::PgTransaction;

use crate::{CompletedFetch, ConsumeError};

/// One ordered provider-resolution step recorded alongside a run's terminal state.
#[derive(Debug)]
pub struct ResolutionStep<'a> {
    /// Zero-based position of this step within the resolution sequence.
    pub ordinal: i32,
    /// Step kind: `provider_attempt`, `resolved_target`, or `html_fallback`.
    pub kind: &'a str,
    /// Outcome label when the step produced one.
    pub outcome: Option<&'a str>,
    /// Failure class when the step failed.
    pub failure_class: Option<&'a str>,
    /// Canonical external URL when the step resolved a target.
    pub resolved_url: Option<&'a str>,
}

pub(super) fn validate_candidates(
    candidates: &[CandidateDecision],
    selected: usize,
) -> Result<(), ConsumeError> {
    let selected_count = candidates
        .iter()
        .filter(|candidate| candidate.selected)
        .count();
    if !candidates.is_empty() && selected_count == selected {
        Ok(())
    } else {
        Err(ConsumeError::InvalidRunState)
    }
}

pub(super) async fn insert_fetch(
    transaction: &mut PgTransaction<'_>,
    run_id: uuid::Uuid,
    fetch: &CompletedFetch<'_>,
) -> Result<(), ConsumeError> {
    let wire_bytes = i64::try_from(fetch.wire_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
    let decoded_bytes =
        i64::try_from(fetch.decoded_bytes).map_err(|_| ConsumeError::InvalidArtifact)?;
    let attempts = i32::try_from(fetch.attempts).map_err(|_| ConsumeError::InvalidArtifact)?;
    sqlx::query(
        "insert into extractor.fetches
             (fetch_id, run_id, final_url, http_status, media_type, wire_bytes, decoded_bytes,
              attempts, cache_outcome, etag, last_modified, fetched_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                 transaction_timestamp())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(run_id)
    .bind(fetch.final_url)
    .bind(i32::from(fetch.http_status))
    .bind(fetch.media_type)
    .bind(wire_bytes)
    .bind(decoded_bytes)
    .bind(attempts)
    .bind(fetch.cache_outcome)
    .bind(fetch.etag)
    .bind(fetch.last_modified)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn insert_candidates(
    transaction: &mut PgTransaction<'_>,
    run_id: uuid::Uuid,
    candidates: &[CandidateDecision],
) -> Result<(), ConsumeError> {
    for candidate in candidates {
        let metrics = serde_json::json!({
            "text_characters": candidate.metrics.text_characters,
            "paragraph_count": candidate.metrics.paragraph_count,
            "text_volume": candidate.metrics.text_volume,
            "paragraph_distribution": candidate.metrics.paragraph_distribution,
            "non_link_share": candidate.metrics.non_link_share,
            "non_boilerplate_share": candidate.metrics.non_boilerplate_share,
            "title_agreement": candidate.metrics.title_agreement,
            "accepted": candidate.accepted,
        });
        let reasons = candidate
            .reasons
            .iter()
            .map(|reason| match reason {
                QualityReason::Accepted => "accepted",
                QualityReason::TooShort => "too_short",
                QualityReason::BelowThreshold => "below_threshold",
            })
            .collect::<Vec<_>>();
        sqlx::query(
            "insert into extractor.candidates
                 (candidate_id, run_id, strategy, extractor_version, metrics, score, reasons,
                  selected, artifact_id, created_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, null, transaction_timestamp())",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(run_id)
        .bind(&candidate.strategy)
        .bind(candidate.evaluator_version)
        .bind(metrics)
        .bind(f64::from(candidate.score) / 1000.0)
        .bind(serde_json::to_value(reasons)?)
        .bind(candidate.selected)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

pub(super) async fn insert_resolution_steps(
    transaction: &mut PgTransaction<'_>,
    run_id: uuid::Uuid,
    steps: &[ResolutionStep<'_>],
) -> Result<(), ConsumeError> {
    for step in steps {
        sqlx::query(
            "insert into extractor.provider_resolutions
                 (step_id, run_id, ordinal, kind, outcome, failure_class, resolved_url, created_at)
             values ($1, $2, $3, $4, $5, $6, $7, transaction_timestamp())",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(run_id)
        .bind(step.ordinal)
        .bind(step.kind)
        .bind(step.outcome)
        .bind(step.failure_class)
        .bind(step.resolved_url)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}
