//! Extractor-owned persistence facts.

use extractor_persistence::{
    ArtifactKind, ArtifactRecord, CandidateRecord, FetchRecord, record_artifact, record_candidate,
    record_fetch,
};
use ratatoskr_identifiers::{
    BlobOwner, BlobRef, ContentDigest, DigestAlgorithm, DigestHex, MediaType,
};

#[tokio::test]
async fn run_artifacts_and_candidates_are_owner_scoped_and_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let database = extractor_persistence::test_support::TestDatabase::create().await?;
    let (run_id, owner_id) = seed_run(database.database.pool()).await?;
    let reference = blob_ref()?;
    let fetch = FetchRecord {
        run_id,
        owner_id,
        final_url: "https://example.com/article",
        http_status: 200,
        media_type: "text/html",
        wire_bytes: 17,
        decoded_bytes: 23,
        attempts: 1,
        cache_outcome: "fresh",
        etag: Some("etag-1"),
        last_modified: None,
    };
    let artifact = ArtifactRecord {
        run_id,
        owner_id,
        kind: ArtifactKind::RawSource,
        reference: &reference,
    };
    let metrics = serde_json::Map::new();
    let candidate = CandidateRecord {
        run_id,
        owner_id,
        strategy: "html_primitives",
        extractor_version: "html-v1",
        metrics: &metrics,
        score: None,
        reasons: &[],
        artifact_id: None,
    };

    for _ in 0..2 {
        record_fetch(database.database.pool(), &fetch).await?;
        record_artifact(database.database.pool(), &artifact).await?;
        record_candidate(database.database.pool(), &candidate).await?;
    }

    let wrong_owner = uuid::Uuid::now_v7();
    let wrong_fetch = FetchRecord {
        owner_id: wrong_owner,
        ..fetch
    };
    let wrong_artifact = ArtifactRecord {
        owner_id: wrong_owner,
        ..artifact
    };
    let wrong_candidate = CandidateRecord {
        owner_id: wrong_owner,
        ..candidate
    };
    let refused = [
        record_fetch(database.database.pool(), &wrong_fetch)
            .await
            .is_err(),
        record_artifact(database.database.pool(), &wrong_artifact)
            .await
            .is_err(),
        record_candidate(database.database.pool(), &wrong_candidate)
            .await
            .is_err(),
    ];

    let mut counts = Vec::new();
    for table in ["fetches", "artifacts", "candidates"] {
        let count: i64 = sqlx::query_scalar(&format!("select count(*) from extractor.{table}"))
            .fetch_one(database.database.pool())
            .await?;
        counts.push((table, count));
    }
    let owner: String = sqlx::query_scalar("select owner_service from extractor.artifacts")
        .fetch_one(database.database.pool())
        .await?;
    let forbidden: i64 = sqlx::query_scalar(
        "select count(*) from information_schema.columns
          where table_schema = 'extractor'
            and table_name in ('fetches', 'artifacts', 'candidates')
            and column_name in ('body', 'bytes', 'content', 'path', 'payload', 'raw_bytes')",
    )
    .fetch_one(database.database.pool())
    .await?;
    database.cleanup().await?;

    assert!(refused.into_iter().all(std::convert::identity));
    for (table, count) in counts {
        assert_eq!(count, 1, "{table}");
    }
    assert_eq!(owner, "ratatoskr-extractor");
    assert_eq!(forbidden, 0);
    Ok(())
}

async fn seed_run(pool: &sqlx::PgPool) -> Result<(uuid::Uuid, uuid::Uuid), sqlx::Error> {
    let command_id = uuid::Uuid::now_v7();
    let owner_id = uuid::Uuid::now_v7();
    let source_id = uuid::Uuid::now_v7();
    let run_id = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into extractor.inbox_events
             (command_id, subject, command_type, producer, received_at, applied_at, outcome)
         values ($1, 'cmd.content.capture.requested.v1', 'content.capture.requested.v1',
                 'ratatoskr-platform', now(), now(), 'applied')",
    )
    .bind(command_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.sources
             (source_id, owner_id, original_url, normalized_url, canonical_url, host,
              classification, created_at)
         values ($1, $2, 'https://example.com/article', 'https://example.com/article',
                 'https://example.com/article', 'example.com', 'generic_web', now())",
    )
    .bind(source_id)
    .bind(owner_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "insert into extractor.extraction_runs
             (run_id, command_id, operation_id, owner_id, correlation_id, source_id, document_id,
              status, policy_version, normalizer_version, parser_version, queued_at, started_at)
         values ($1, $2, $3, $4, $5, $6, $7, 'running', 'ssrf-v1', 'url-v1', 'html-v1', now(), now())",
    )
    .bind(run_id)
    .bind(command_id)
    .bind(uuid::Uuid::now_v7())
    .bind(owner_id)
    .bind(format!("operation:{}", uuid::Uuid::now_v7()))
    .bind(source_id)
    .bind(uuid::Uuid::now_v7())
    .execute(pool)
    .await?;
    Ok((run_id, owner_id))
}

fn blob_ref() -> Result<BlobRef, Box<dyn std::error::Error>> {
    Ok(BlobRef {
        owner_service: BlobOwner::parse("ratatoskr-extractor")?,
        digest: ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            hex: DigestHex::parse(&"a".repeat(64))?,
        },
        media_type: MediaType::parse("text/html")?,
        length_bytes: 23,
    })
}
