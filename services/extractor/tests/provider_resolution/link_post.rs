use super::*;

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one hermetic scenario starts servers, queues one run and verifies every persisted outcome"
)]
async fn hn_link_post_completes_from_resolved_article() -> Result<(), Box<dyn std::error::Error>> {
    let article_server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(format!(
            "<!doctype html><html><head><title>Fixture article</title></head>\
             <body><article><h1>Fixture article</h1>\
             <p>{ARTICLE_MARKER}</p>\
             <p>The resolved link post points at this canonical external article, so its body \
             carries steady paragraph volume for the shared evaluator while every byte remains \
             deterministic across repeated runs and no live network takes part in the check.</p>\
             <p>A second padded paragraph keeps the text-to-markup balance comfortable for the \
             recorded acceptance thresholds, mirroring how the discussion fixtures in this file \
             are padded without touching any recorded assertion about the stored document.</p>\
             </article></body></html>"
        ))])
        .with_header(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html"),
        ),
    ])
    .await?;
    let article_url = article_server
        .uri("/article")
        .replace("127.0.0.1", "localhost");
    let algolia_payload = json!({
        "id": 1000, "title": "Fixture link post", "text": null, "url": article_url,
        "children": [
            { "id": 1001, "text": fixture_comment(DISCUSSION_MARKER_A), "children": [] },
            { "id": 1002, "text": fixture_comment(DISCUSSION_MARKER_B), "children": [] },
        ],
    });
    let algolia_server = ScriptedServer::start(vec![
        ScriptedResponse::chunks([bytes::Bytes::from(serde_json::to_vec(&algolia_payload)?)])
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            ),
    ])
    .await?;
    let database = TestDatabase::create().await?;
    let root = TemporaryBlobRoot::create().await?;
    let store = BlobStore::new(root.path());
    let pool = database.database.pool();
    let mut config = ExtractorConfig::built_in(root.path());
    config.fetch.allowed_ports = vec![algolia_server.port(), article_server.port()];
    config.fetch.total_timeout_ms = 10_000;
    let fetcher = SafeFetcher::new_for_test(config.fetch.clone(), store.clone())?;
    let address = algolia_server
        .uri("/api/v1/items/1000")
        .replace("127.0.0.1", "localhost");
    queue_direct(
        pool,
        "https://news.ycombinator.com/item?id=1000",
        "hacker_news",
    )
    .await?;
    let run = claim_queued_run(pool, "test-worker", 60)
        .await?
        .ok_or("the queued run did not lease")?;
    extractor_service::_complete_provider_for_test(
        pool,
        &store,
        &config.providers,
        &fetcher,
        &config.parser,
        extractor_providers::SourceRoute::HackerNews,
        &address,
        &run,
    )
    .await?;
    let baseline: (String, Option<String>, i64, i64, i64) = sqlx::query_as(
        "select r.status, r.last_error_class,
            (select count(*) from extractor.candidates c where c.run_id = r.run_id and c.selected and c.strategy = 'hacker_news_item'),
            (select count(*) from extractor.artifacts a where a.run_id = r.run_id and a.kind = 'document_ir'),
            (select count(*) from extractor.outbox_events o where o.subject = 'evt.content.document.extracted.v1')
           from extractor.extraction_runs r where r.run_id = $1",
    ).bind(run.run_id).fetch_one(pool).await?;
    assert_eq!(baseline.0, "succeeded");
    assert_eq!(baseline.1, None);
    assert_eq!(baseline.2, 1, "one selected hacker_news_item candidate");
    assert_eq!(baseline.3, 1);
    assert_eq!(baseline.4, 1);
    let (fetch_total, resolved_fetches): (i64, i64) = sqlx::query_as(
        "select count(*), count(*) filter (where f.final_url = $2) from extractor.fetches f where f.run_id = $1",
    ).bind(run.run_id).bind(&article_url).fetch_one(pool).await?;
    assert_eq!(fetch_total, 2, "provider payload plus resolved article");
    assert_eq!(resolved_fetches, 1);
    let resolutions: Vec<(i32, String, Option<String>)> = sqlx::query_as(
        "select ordinal, kind, resolved_url from extractor.provider_resolutions where run_id = $1 order by ordinal",
    ).bind(run.run_id).fetch_all(pool).await?;
    let resolved_urls: Vec<Option<String>> =
        resolutions.iter().map(|step| step.2.clone()).collect();
    let kinds: Vec<&str> = resolutions.iter().map(|step| step.1.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["provider_attempt", "resolved_target"],
        "the attempt and the resolution must both be recorded"
    );
    assert_eq!(
        resolved_urls[1].as_deref(),
        Some(article_url.as_str()),
        "the resolved target must carry the canonical article URL"
    );
    let (raw_sources,): (i64,) = sqlx::query_as(
        "select count(*) from extractor.artifacts where run_id = $1 and kind = 'raw_source'",
    )
    .bind(run.run_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(raw_sources, 2);
    let ir_row: (String, String, String, String, i64) = sqlx::query_as(
        "select owner_service, digest_algorithm, digest_hex, media_type, length_bytes from extractor.artifacts where run_id = $1 and kind = 'document_ir'",
    ).bind(run.run_id).fetch_one(pool).await?;
    let ir_ref: BlobRef = serde_json::from_value(json!({
        "owner_service": ir_row.0, "digest": {"algorithm": ir_row.1, "hex": ir_row.2},
        "media_type": ir_row.3, "length_bytes": u64::try_from(ir_row.4)?,
    }))?;
    let ir_path = store.verify(&ir_ref).await?;
    let document_text = tokio::fs::read_to_string(ir_path).await?;
    assert!(
        document_text.contains(ARTICLE_MARKER),
        "the document must carry the external article body"
    );
    assert!(
        !document_text.contains(DISCUSSION_MARKER_A)
            && !document_text.contains(DISCUSSION_MARKER_B),
        "the document must not fall back to the discussion conversion"
    );
    assert_eq!(
        article_server.request_count(),
        1,
        "the resolved article is fetched exactly once"
    );
    database.cleanup().await?;
    Ok(())
}
