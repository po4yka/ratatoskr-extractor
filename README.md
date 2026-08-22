# Ratatoskr Extractor

`ratatoskr-extractor` turns external URLs and files into deterministic, provenance-preserving documents for Ratatoskr. Its core design is **fetch once, parse once, score deterministically, and escalate to a browser only when necessary**.

> **Status:** implementation plan items 1 through 6 are complete plus direct PDF extraction (the
> first half of item 7): foundation, URL/SSRF policy, bounded retrieval, parse-once HTML
> candidates, deterministic quality selection, Document IR, the durable PostgreSQL/JetStream
> pipeline, and bounded PDF text extraction. Provider/source adapters and the browser worker remain
> planned; scanned PDFs without a text layer are recorded as an explicit degraded outcome.

> [!IMPORTANT]
> **Ratatoskr is in development.** No database holds data that has to survive a schema change.
> While this status holds, these two rules replace what the documents below plan:
>
> - the API and the database keep their first version. There is no `v2` and no later major
>   version.
> - the database has no migrations. One schema definition exists, and a schema change edits it in
>   place.
>
> Only the repository owner changes this status.

## Role in Ratatoskr

The legacy backend relies on a long fallback chain of overlapping HTTP, browser, sidecar, and LLM-driven scraper frameworks. Ratatoskr replaces that topology with one owned extraction pipeline and a separate, isolated Chromium process.

This repository is expected to provide two deployables:

```text
ratatoskr-extractor
ratatoskr-browser-worker
```

The extractor owns URL classification, safe retrieval, parsing, candidate generation, quality scoring, and canonical document construction. The browser worker owns Chromium lifecycle and rendered-DOM acquisition. Neither component performs summaries, embeddings, or semantic interpretation.

## Implemented workspace

```text
crates/core          typed configuration and startup errors
crates/telemetry     structured tracing and Prometheus recording
crates/url-routing   URL identity, source classification, and SSRF-safe DNS
crates/blob-store    extractor-owned content-addressed artifacts and BlobRef verification
crates/safe-fetch    pooled HTTP, redirects, limits, decoding, cache validation, and retry
crates/document-ir   bounded HTML5 parse-once candidates, quality selection, and shared Document IR
crates/persistence   finite PostgreSQL pool and the one editable extractor schema
crates/eventing      typed command inbox, leased worker records, outbox, and JetStream ACKs
crates/test-support  deterministic local network and storage fixtures
services/extractor   command consumer, fetch/parse worker, publisher, health, and joined shutdown
deploy/systemd       production unit and environment example
```

The service exposes only `/health/live`, `/health/ready`, `/metrics`, and `/version`; capture work
arrives through `cmd.content.capture.requested.v1`, not an HTTP fetch API.

## Target pipeline

```text
URL or file
  -> normalize and classify
  -> cache lookup
  -> SSRF-safe HTTP fetch
  -> MIME and source detection
  -> one DOM or document parse
  -> multiple in-memory extraction candidates
  -> deterministic quality scoring
  -> accept or browser escalation
  -> canonical Document IR
  -> content.document.extracted.v1
```

The ordinary path performs one network fetch. Candidate extractors compete over the same parsed document rather than independently downloading or rendering the source.

## Source classification

Routing happens before generic article extraction. Direct PDF extraction is implemented; remaining
planned source adapters include:

- Reddit public/API representations;
- Hacker News item data;
- ordinary HTML articles;
- YouTube transcript/media references;
- platform URLs routed to GitHub, X, Instagram, Threads, Telegram, or AI-archive services;
- user-supplied text and files.

Platform-specific services remain authoritative for authenticated account data. The extractor processes generic public content and files; it does not store provider credentials.

## Safe HTTP retrieval

The implemented retrieval layer uses `reqwest` with `rustls` and enforces:

- HTTP/HTTPS-only targets;
- DNS and redirect-hop SSRF checks;
- blocking of loopback, private, link-local, metadata, reserved, and unsafe address ranges;
- explicit distinction between DNS failures and policy blocks;
- response-size and decompression limits;
- connection pooling and keep-alive;
- global and per-host concurrency budgets;
- streaming hashes and MIME sniffing;
- bounded redirects and timeouts;
- `Retry-After`, ETag, and `Last-Modified` support;
- closed retry and failure classification.

Raw response metadata and body hashes are retained as provenance. Sensitive URL components are redacted from logs.

## Parse once, extract many

HTML is parsed once using an HTML5 parser. Item 4 emits headings and paragraphs with block-level
raw-blob provenance. Item 5 runs three deterministic candidates over that same DOM:

- semantic `<article>` and `<main>` extraction;
- Readability-compatible extraction;
- text-density extraction;

Each candidate records bounded evidence and integer score components. The highest accepted score
wins; ties prefer semantic, readability, then density. JSON-LD and source-specific strategies remain
deferred.

## Canonical Document IR

Markdown is an output format, not the canonical internal representation. The extractor publishes
the shared `ratatoskr-document-contracts::Document`; its current block variants are:

```rust
pub enum DocumentBlock {
    Heading { level: u8, text: String },
    Paragraph { text: String },
}
```

The IR enables later generation of:

- sanitized reader HTML;
- Markdown exports;
- plain text for search and embeddings;
- bounded LLM context;
- block-level citations;
- future renderers without re-fetching the source.

Provider-specific information that cannot yet be represented remains available through raw blob references and extension metadata.

## Quality scoring

Acceptance is driven by deterministic, explainable signals rather than "first provider to return
text". Evaluator `quality_v1` assigns up to 1000 points from:

- text volume (300);
- paragraph distribution (200);
- non-link share (200);
- non-boilerplate share (200);
- title/body agreement (100).

Acceptance requires at least 120 normalized characters and a score of at least 350. Every terminal
transaction stores all three decisions and exactly one selected marker on success; a low-quality
failure stores three unselected decisions and no Document IR event.

An extraction result records the selected strategy and score explanation, for example:

```text
strategy = semantic
score = 0.68
text_characters = 132
reasons = ["accepted"]
```

Thresholds are calibrated against a golden corpus, not treated as permanent magic constants.

## Browser escalation

The browser worker is invoked only when static retrieval cannot produce acceptable content, such as:

- an empty JavaScript shell;
- hydration-dependent article content;
- a known host policy requiring rendering;
- a permitted user-session-backed operation;
- a deterministic quality score below the browser threshold.

The worker:

- reuses a Chromium process while isolating each job in a fresh context;
- blocks unnecessary images, video, fonts, trackers, and ads when possible;
- enforces navigation, resource, memory, CPU, and wall-clock limits;
- returns rendered DOM, final URL, response metadata, and network evidence;
- does not run article interpretation or LLM agents.

Browser control remains behind a replaceable `BrowserRenderer` interface. Multiple expensive browser engines are not raced by default.

## Caching and strategy learning

Planned cache keys include normalized URL, redirect-resolved canonical URL, content hash, extractor version, and relevant request policy. Conditional requests avoid downloading unchanged content.

A bounded domain-strategy cache may record:

```text
host
preferred_strategy
success_rate
median_quality
p50_latency
p95_latency
browser_escalation_rate
last_failure_class
expires_at
```

This is an optimization hint, not an authority. Every result still passes the same validation gates.

## Data ownership

Raw bytes stay on the extractor host beneath its configured private content-addressed root. The
artifact is announced with the shared `BlobRef` contract, which carries owner, SHA-256 digest,
effective media type, and exact length. It carries no filesystem path and does not cause an HTTP
hop to local storage.

Item 6 adds the editable `schema.sql`, inbox/run/fetch/artifact/outbox records, and acknowledged
JetStream delivery. There is no migration ledger while the product remains in development.

## Events

Implemented bus subjects are:

```text
cmd.content.capture.requested.v1
evt.content.document.extracted.v1
evt.platform.operation.reported.v1
```

Events are idempotent and correlated with Platform operations. Knowledge consumes accepted documents; it does not depend on extractor database tables.

## Observability

Core metrics include:

```text
extractor_request_duration
extractor_fetch_bytes
extractor_candidate_score
extractor_selected_strategy
extractor_browser_escalation_rate
extractor_cache_hit_rate
extractor_ssrf_blocks
extractor_dns_failures
extractor_quality_rejections
browser_job_duration
browser_process_restarts
```

Attempt-level diagnostics must support comparison with the legacy pipeline during shadow mode.

## Golden corpus and migration

Item 5 includes four minimized synthetic calibration fixtures: semantic, noisy, malformed, and
login HTML. Plan item 9 will expand this to a representative corpus covering:

- static and malformed HTML;
- JavaScript applications;
- paywalls and consent walls;
- Russian, English, and French content;
- code-heavy articles and tables;
- long reads;
- PDFs;
- deleted and error pages;
- anti-bot responses.

The legacy and Rust pipelines will run in shadow mode. Evaluation compares completeness, boilerplate, p50/p95 latency, network requests, browser launches, downloaded bytes, CPU time, memory, and failure classification. Cutover proceeds by source class rather than waiting for universal support.

## Security invariants

1. Every target is validated before retrieval and after redirects.
2. Delegated browser work never receives unrelated provider credentials.
3. Response and decompression sizes are bounded.
4. Active content is never executed outside the isolated browser worker.
5. Raw content is treated as untrusted input.
6. LLMs are not part of the deterministic extraction path.
7. A failed or partial candidate cannot silently replace a previously verified document.
8. Logs and metrics do not expose secrets or sensitive query parameters.

## Non-goals

- Summarization, entity extraction, embeddings, or semantic search.
- GitHub account synchronization.
- X, Instagram, Threads, ChatGPT, Claude, or Telegram credential ownership.
- Stealth login automation or bypassing private-content access controls.
- Running every available scraper or browser in parallel.
- Treating Markdown as the sole source of truth.

## Initial milestones

1. Establish contracts, service skeleton, configuration, and the editable schema.
2. Implement URL normalization and SSRF-safe HTTP fetching.
3. Add HTML5 parsing and the first extraction candidates.
4. Define and persist Document IR.
5. Add quality scoring and golden-corpus benchmarks.
6. Introduce direct PDF extraction.
7. Add the isolated browser worker.
8. Run legacy shadow comparisons and cut over static HTML first.

## Workspace integration

`ratatoskr-workspace` pins this repository with compatible Document contracts, Knowledge consumers, and integration fixtures. The extractor remains independently buildable and testable, including corpus benchmarks that do not require the full system.

## Project status

The ordinary HTML service path, parser, deterministic evaluator, small calibration corpus, and
direct PDF extraction with typed encrypted/pathological failure modes are implemented. Provider
adapters, the isolated browser worker, OCR for scanned PDFs, and broad corpus/performance reporting
remain planned.
