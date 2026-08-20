# Ratatoskr Extractor

`ratatoskr-extractor` turns external URLs and files into deterministic, provenance-preserving documents for Ratatoskr. Its core design is **fetch once, parse once, score deterministically, and escalate to a browser only when necessary**.

> **Status:** architecture bootstrap. The extraction engine, browser worker, Document IR, and benchmark corpus described below are planned and are not implemented yet.

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

Routing happens before generic article extraction. Planned source adapters include:

- Reddit public/API representations;
- Hacker News item data;
- direct PDFs;
- ordinary HTML articles;
- YouTube transcript/media references;
- platform URLs routed to GitHub, X, Instagram, Threads, Telegram, or AI-archive services;
- user-supplied text and files.

Platform-specific services remain authoritative for authenticated account data. The extractor processes generic public content and files; it does not store provider credentials.

## Safe HTTP retrieval

The retrieval layer is expected to use `reqwest` with `rustls` and enforce:

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
- circuit breaking and structured failure classification.

Raw response metadata and body hashes are retained as provenance. Sensitive URL components are redacted from logs.

## Parse once, extract many

HTML is parsed once using a browser-grade HTML5 parser. Multiple deterministic algorithms then produce candidates from the same DOM:

- Readability-compatible extraction;
- semantic `<article>` and `<main>` extraction;
- text-density extraction;
- JSON-LD `articleBody` extraction;
- source- or publisher-specific selectors;
- metadata and canonical-link extraction.

A candidate contains its blocks, metadata, diagnostics, and evidence. It does not become the accepted document until it passes shared quality gates.

## Canonical Document IR

Markdown is an output format, not the canonical internal representation. The extractor publishes a typed document composed of ordered blocks and source spans:

```rust
pub struct Document {
    pub metadata: DocumentMetadata,
    pub blocks: Vec<Block>,
    pub provenance: Vec<SourceSpan>,
}

pub enum Block {
    Heading { level: u8, text: String },
    Paragraph { text: String },
    List { ordered: bool, items: Vec<String> },
    Quote { text: String },
    Code { language: Option<String>, text: String },
    Table { rows: Vec<Vec<String>> },
    Image { url: String, alt: Option<String> },
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

Acceptance is driven by deterministic, explainable signals rather than "first provider to return text". Candidate scoring may include:

- character, word, paragraph, and sentence counts;
- text-to-DOM ratio;
- link and boilerplate density;
- duplicate and navigation block ratios;
- title/body consistency;
- author and publication metadata;
- error, tombstone, login, consent, and paywall markers;
- suspicious truncation or incomplete endings;
- JSON-LD/DOM agreement;
- language and structural consistency.

An extraction result records the selected strategy and score explanation, for example:

```text
extractor = readability
score = 0.87
words = 1842
link_density = 0.03
boilerplate_ratio = 0.08
browser_used = false
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

The service owns an `extractor.*` PostgreSQL schema for:

- extraction requests and attempts;
- source fetch metadata;
- candidate diagnostics;
- accepted document references;
- host strategy statistics;
- cache and revalidation metadata;
- outbox/inbox records.

Large bodies, rendered DOM, PDFs, attachments, and Document IR snapshots are stored in the shared content-addressed BlobStore under extractor-owned references.

## Events

Expected commands and events include:

```text
content.extraction.requested.v1
content.document.extracted.v1
content.extraction.failed.v1
content.browser_render.requested.v1
content.browser_render.completed.v1
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

Before cutover, the repository will maintain a representative corpus covering:

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

1. Establish contracts, service skeleton, configuration, and migrations.
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

This README defines the intended extraction bounded context. No production engine, browser worker, parser, or benchmark suite is present yet.
