## Context

Intake already classifies `news.ycombinator.com` and Reddit hosts into `SourceRoute::HackerNews` /
`SourceRoute::Reddit` and stores the classification on the source row, but the claimed run carries
only the URL, so the pipeline routes every non-PDF response to the HTML path. The direct PDF change
established the single-strategy completion pattern (one candidate, explicit failure classes,
`evaluate_plain_text`, `assemble_document`) that provider adapters reuse.

## Goals / Non-Goals

Goals: deterministic JSON-to-IR conversion for the two classified providers; one network operation
per provider run; typed failure modes for anti-bot or schema-invalid responses; reuse of the shared
completion transaction, evaluator, and events; offline tests against synthetic fixtures.

Non-Goals: YouTube transcripts; GitHub, X, Instagram, Threads, Telegram, or any authenticated
provider surface (delegated to owning services); OAuth or bookmark synchronization; provider-specific
scoring thresholds; rewriting normalized URLs stored on sources; changing HTML/PDF routing.

## Decisions

### Fetch mapping lives beside parsing in `crates/providers`

`provider_request(classification, normalized_url) -> Option<DocumentAddress>` maps an item URL to
`https://hn.algolia.com/api/v1/items/{id}` and a Reddit comment permalink to itself plus `.json`.
The mapping is pure, unit-tested, and fails closed: any URL outside the documented shapes returns
`None` and the service takes the ordinary HTML path with the original URL. Mapping before fetching
keeps a provider run at exactly one request instead of scraping HTML first and re-fetching JSON;
this is the "protocol correctness" exception to fetch-once, satisfied by never fetching twice.

The mapped URL is fetched through the same `SafeFetcher`, so SSRF policy, redirects, budgets, and
provenance apply unchanged. The original normalized URL stays on the source row as canonical
identity; provenance records the mapped fetch address as the final URL of the one performed request.

### One adapter entry point, two schema parsers

`from_provider(input, limits) -> Result<ProviderExtraction, ProviderError>` dispatches on
classification. Each parser uses serde with tolerant defaults but required identity fields
(`id`/`title` for HN stories; listing kind tags and post `id`/`title` for Reddit), so missing or
renamed fields fail as typed schema errors rather than silently empty documents. HN `text` fields
carry legacy HTML entities and inline tags; they are reduced to plain text through the existing
HTML DOM text extraction exposed from `document-ir`, avoiding a hand-rolled entity decoder.

Blocks: one Heading from the story/post title, then Paragraphs for body text and each visible
comment body in API pre-order, capped by budget. The title also feeds the document title and the
shared evaluator's agreement component via `evaluate_plain_text`.

### Failure classes

Media type other than `application/json` on the provider branch → `provider_response`;
schema-invalid JSON → `provider_response`; budget overruns → `parse`; transport failures keep their
existing retryable classes. Anti-bot HTML challenges arrive exactly as the non-JSON case, so they
can never be misread as articles.

### Claimed runs carry classification

`claim_queued_run` joins the source row it already reads and extends `QueuedRun` with
`classification: String`. Intake parser-version mapping gains the two provider routes →
`providers-v1`. No schema change: both values already exist in columns.

### Budgets

`ProvidersConfig { max_input_bytes, max_blocks }` defaults 8 MiB / 2,000, validated like existing
sections and enforced inside the adapters before block construction grows unbounded.

## Risks / Trade-offs

[Provider APIs drift] → required-field parsing fails closed with typed errors; fixtures pin today's
shapes so drift surfaces in tests before production.
[HN Algolia is a second host] → it is the sanctioned public API for item content; the single mapped
request replaces the HTML scrape rather than adding to it.
[Comment-heavy threads produce huge IR] → block budget caps construction; deeper truncation is a
quality decision deferred to golden-corpus work.
[Fallback hides mapping bugs] → fallback only triggers for genuinely unmapped shapes; mapped-shape
failures stay typed failures, and the end-to-end test asserts the no-second-request invariant.

## Migration Plan

Additive branch in the worker loop. Provider URLs previously failed through generic HTML quality
gates or produced scraped approximations; after this change they complete from native JSON.
Rollback is reverting the deploy; no persisted state depends on the new path.

## Open Questions

None blocking; comment-depth policy and YouTube transcripts belong to follow-up changes.
