# Ratatoskr Extractor Agent Instructions

## Scope

These instructions apply to the `ratatoskr-extractor` repository and its planned deployables:

- `ratatoskr-extractor` for deterministic fetch, parse, and extraction;
- `ratatoskr-browser-worker` for isolated Chromium rendering.

More specific instructions in subdirectories may tighten implementation rules but must preserve the repository-wide invariants below.

## Repository mission

The repository converts untrusted external URLs and documents into a stable, provenance-preserving `Document IR`.

Its primary architecture is:

```text
normalize/classify
  -> cache lookup
  -> one safe fetch
  -> one parse
  -> multiple in-memory extraction candidates
  -> deterministic quality scoring
  -> optional browser escalation
  -> canonical Document IR
```

The extractor establishes what content was obtained and how. It does not interpret that content with an LLM.

## Current phase

The repository is in architecture bootstrap. Do not assume Rust crates, parsers, Chromium integration, migrations, golden fixtures, or CI commands exist unless they are present in the checkout.

When adding initial scaffolding:

- preserve separate extractor and browser-worker process boundaries;
- keep extraction algorithms behind narrow traits;
- make safe defaults mandatory rather than optional call-site flags;
- avoid adding several overlapping scraper frameworks as shortcuts.

### Development status

Ratatoskr is in development. No database holds data that has to survive a schema change. While this
status holds, these rules are binding, and they override anything else in this repository that
plans otherwise, including the rest of this file:

- **One version only.** The API, the database, and the contracts keep their first version. Do not
  add a `v2` or a later major version, and do not add version negotiation, deprecation windows, or
  parallel-major routing.
- **No database migrations.** Do not add a migration file, and do not add migration tooling. A
  schema change edits the current schema definition in place, and a test database is created from
  that definition.
- **The product is `Ratatoskr`.** It is not "Ratatoskr Next". Do not write that name in code,
  documentation, identifiers, comments, or commit messages.

Only the repository owner changes this status. Ask before you write anything these rules forbid.

## How a change starts

Every non-trivial change begins as an OpenSpec change rather than as an edit. In your assistant that
is `/opsx:propose <what you want to build>`, or `/opsx:explore` first when the shape is not clear
yet. The command writes `openspec/changes/<id>/` holding a proposal, the spec deltas, a design and a
task list, and you read that plan before any code is written. `/opsx:apply` builds it and
`/opsx:archive` folds the deltas into `openspec/specs/`.

`openspec/specs/` holds the behaviour that is true today, and it starts empty on purpose. A spec here
grows from a change that needed it. Do NOT convert `docs/REQUIREMENTS.md`, `docs/INTERFACES.md`,
`docs/DOMAIN.md` or `docs/DATA_MODEL.md` into specs in bulk. Those documents stay where they are, as
material an exploration reads. A spec set produced by bulk conversion is large, stale on the day it
lands, and trusted by nobody.

Behaviour that more than one repository can see — the shape of a contract, the meaning of a field, the
order in which repositories must receive a change — belongs in the `ratatoskr-workspace` store, not
here. `openspec/config.yaml` references it, so `openspec instructions` in this repository lists the
store's specs with the exact command that fetches one. Cite that spec from a local proposal instead
of restating it.

### Tests come first

The task list carries one pair per behaviour. The first task adds a test that fails. The second makes
it pass. Never one task that does both.

- Run the new test before you write the implementation, and confirm it fails for the reason the task
  states — not for a compile error or a typo.
- A refactor task comes after the tests are green. It adds no test and changes no behaviour.
- A task that cannot start from a failing test says why in one line. Configuration, documentation and
  generated files are the usual reasons.
- Do not tick a task whose test has not been run.

Nothing can check the order in which the two were written. What CI does check is
`openspec validate --archived`, which fails when a change was archived with a task left unticked, and
the step in `fleet.yml` that fails when a repository holds a manifest and a `ci.yml` that never runs
a test. `ratatoskr-workspace/docs/QUALITY_GATES.md` states that limit rather than implying it is
covered.

## Sources of truth

Use this order:

1. active task/changeset and accepted ADRs;
2. `README.md`;
3. Document IR and event contracts from `ratatoskr-contracts`;
4. golden-corpus expectations and repository tests;
5. implementation details.

A provider library's default behavior never overrides Ratatoskr security, quality, provenance, or resource-budget rules.

## Hard bounded-context rules

### Extractor owns

- URL normalization and classification;
- network safety policy for extraction fetches;
- raw response/document blob references;
- HTML/PDF parsing;
- extraction candidates and deterministic quality scores;
- browser escalation decisions;
- canonical Document IR production;
- extraction provenance and diagnostics;
- host strategy observations and extraction cache metadata.

### Extractor does not own

- LLM summaries, tags, entities, or embeddings;
- semantic search indices;
- provider OAuth accounts or bookmark synchronization;
- GitHub catalog or Git backup state;
- Telegram interaction state;
- general-purpose browser automation for logging into user accounts;
- content collections or client presentation state.

Never add an LLM fallback to the deterministic extraction path. Send an extracted document to `ratatoskr-knowledge` after extraction completes.

## Fetch-once / parse-once invariant

The ordinary path must avoid duplicated network and DOM work.

- Fetch a URL once per resolved extraction attempt unless protocol correctness requires another request.
- Parse a body once into a shared representation.
- Run extraction candidates against the same parsed document.
- Do not race several frameworks that each repeat DNS, HTTP, DOM construction, or Chromium startup.
- A retry must be explicit, bounded, observable, and justified by an eligible transient failure.
- Cache and conditional request behavior must preserve content identity and provenance.

Hedged requests are exceptional. They require measured latency evidence, a bounded delay, idempotent cheap operations, and cancellation accounting. Browser, remote paid extraction, and LLM agents are not default hedge candidates.

## URL normalization and routing

Normalization must preserve enough information to audit the original request while producing a stable canonical routing key.

- Preserve the original URL separately from the normalized/canonical URL.
- Normalize scheme, host casing, default ports, fragments, and known tracking parameters according to documented rules.
- Validate every redirect target, not only the first URL.
- Route known sources before generic HTML when an authoritative structured adapter exists.
- Do not route private or authenticated provider content through a generic scraper when a provider service owns it.
- Keep canonicalization rules versioned when they affect hashes, deduplication, or cache keys.

Source-specific adapters may include PDFs, Reddit, Hacker News, YouTube transcripts, or other explicitly supported public sources. GitHub repository URLs and social-platform URLs should be delegated to their owning services when appropriate.

## SSRF and network safety

All network input is untrusted.

Mandatory controls include:

- scheme allowlisting;
- host and port policy;
- DNS resolution checks;
- blocking loopback, link-local, private, multicast, metadata-service, and otherwise prohibited ranges;
- redirect-hop revalidation;
- protection against DNS rebinding assumptions;
- request and total-operation timeouts;
- response body and decompression limits;
- connection, redirect, and retry limits;
- per-host and global concurrency budgets;
- safe proxy configuration;
- no ambient access to internal service networks unless explicitly designed.

Distinguish policy blocks from DNS failures and remote HTTP failures. Do not leak internal address information in user-facing errors.

Tests must cover IPv4, IPv6, encoded/alternative address forms, redirects, and resolution changes where practical.

## HTTP retrieval rules

- Use a shared connection pool and a consistent user agent policy.
- Stream large bodies and compute content hashes without unnecessary full-memory copies.
- Enforce limits before allocation when possible.
- Respect `Retry-After`, ETag, and `Last-Modified` where semantically valid.
- Record final URL, status, MIME evidence, headers needed for provenance, byte counts, and timings.
- Do not persist authorization headers, cookies, or secret query values in diagnostics.
- Treat server-declared MIME as evidence, not unquestioned truth; sniff safely within bounded input.
- Detect error pages and anti-bot responses without misrepresenting them as extracted articles.

Retries are reserved for eligible transient failures. Do not retry policy blocks, deterministic parser failures, unsupported formats, or confirmed permanent HTTP responses.

## Parsing rules

- Prefer standards-compliant parsers for malformed real-world input.
- Keep raw bytes/blob references so a document can be reprocessed by a newer parser.
- Do not render active HTML in the extractor process.
- Sanitize output at rendering boundaries; Document IR should preserve content structure without executable behavior.
- Bound parser depth, node count, table size, image count, and pathological input behavior.
- Move CPU-heavy parsing or compression work to a bounded blocking/CPU pool rather than blocking async runtime workers.

PDF extraction must preserve page/provenance information where available and apply file size, object count, decompression, and parser safety limits.

## Extraction candidates

Candidate extractors should be narrow and deterministic, for example:

- Readability-compatible extraction;
- semantic `<article>`/`<main>` extraction;
- text-density extraction;
- publisher-specific selectors;
- sufficiently complete JSON-LD article bodies.

Each candidate returns structured content plus evidence and diagnostics. Candidates do not fetch the URL themselves.

Avoid provider-specific hacks in the common core. Put host rules in explicit adapters or strategy configuration with fixtures.

## Quality scoring

Acceptance must be based on an explainable deterministic score, not merely "non-empty text" or whichever provider finishes first.

Useful signals include:

- text volume and paragraph distribution;
- text-to-DOM ratio;
- link and boilerplate density;
- title/body consistency;
- duplicate blocks;
- navigation/footer/related-content ratio;
- error, paywall, consent, and challenge markers;
- sentence structure;
- suspicious truncation;
- metadata completeness;
- agreement between structured metadata and DOM content.

Store the selected candidate, component scores, thresholds, and escalation reason. Threshold changes require golden-corpus evaluation.

Do not tune scoring solely against one failing site if the change degrades broader quality.

## Document IR rules

Document IR is the canonical normalized output. Markdown is a derived export, not the sole storage format.

When producing IR:

- preserve block order;
- use stable typed block variants;
- preserve headings, paragraphs, lists, quotes, code, tables, and images when supported;
- attach source/provenance spans or references;
- distinguish metadata extracted from provider/HTML from inferred normalization;
- avoid executable HTML, JavaScript, event handlers, and unsafe URLs;
- apply deterministic normalization before hashing;
- preserve unknown/extension data only through a documented safe mechanism;
- do not embed summaries or embeddings.

Changes to IR serialization or canonical hashing require a coordinated contract changeset and reprocessing/migration plan.

## Browser escalation

Chromium is a high-cost, high-risk fallback. Escalate only when deterministic evidence indicates it is required, such as:

- an empty JavaScript shell;
- content populated after hydration;
- a known browser-required host strategy;
- a low quality score after safe direct extraction;
- an explicitly authorized session-backed operation designed outside the generic public path.

Browser-worker rules:

- run in a separate process/container/security domain;
- reuse the browser process but isolate jobs in fresh contexts;
- enforce navigation, total, CPU, memory, process, and network limits;
- block unnecessary images, fonts, video, ads, and trackers when not required;
- validate all browser network destinations using equivalent SSRF policy;
- return rendered DOM and network evidence, not an interpreted article;
- clean contexts, storage, downloads, and temporary files after each job;
- never receive social/provider OAuth tokens or user browser cookies from extensions;
- never use stealth or anti-bot bypass as a default product capability.

The same parser and quality scorer should process browser-rendered DOM where possible.

## Caching and content identity

- Cache by normalized request plus the factors that materially change representation.
- Record parser, normalizer, strategy, and IR schema versions.
- Keep raw source hash separate from normalized document hash.
- Do not reuse stale cached content when the caller explicitly requires revalidation.
- Conditional `304` responses must resolve to a known verified prior body.
- Failed/challenge responses must not poison successful content cache entries.
- Reprocessing from raw blobs should not require a new network request.

## Host strategy observations

Host-level optimization may record:

- preferred strategy;
- success and quality rates;
- latency percentiles;
- browser escalation rate;
- failure classes;
- expiry/version.

This data guides ordering but must not disable safety checks or permanently lock a host to a failing strategy. Strategy changes need bounded expiry and fallback.

## Persistence and events

Extractor writes only its owned schema. Cross-schema writes and foreign keys are forbidden.

Published results should reference:

- stable document ID;
- original and final/canonical URLs;
- raw blob reference/hash;
- normalized document hash;
- IR schema/version;
- extraction strategy and quality;
- provenance;
- result status and diagnostics reference.

Use transactional outbox and idempotent processing. Replayed extraction requests with the same idempotency/content identity must not create uncontrolled duplicates.

## Security and privacy

- Never log response bodies, cookies, auth headers, full secret query strings, or uploaded private content by default.
- Apply retention and access controls to raw blobs.
- Treat HTML, PDFs, archives, images, and filenames as malicious input.
- Do not execute embedded scripts, macros, binaries, or document actions.
- Do not expose internal storage paths or network topology in public errors.
- Validate blob references and ownership before reprocessing.
- Keep browser-worker egress and filesystem access minimal.

## Observability

Required telemetry should include:

- fetch/parse/extract durations;
- bytes downloaded and decompressed;
- candidate scores and selected strategy;
- browser escalation reason/rate;
- cache hit/revalidation results;
- SSRF/policy block class;
- retry count and failure class;
- CPU/blocking-pool time;
- peak or bounded resource signals where available;
- operation/correlation IDs.

Avoid unbounded host/URL labels in metrics. Use structured logs or sampled traces for detailed diagnostics.

## Golden corpus and testing

A material extraction change must be evaluated against representative fixtures, including:

- static articles;
- malformed HTML;
- SPAs and hydration-required pages;
- Russian, English, and French content;
- long reads, code-heavy pages, tables, and media;
- PDFs;
- error, deletion, paywall, consent, and anti-bot pages;
- SSRF and redirect attack cases.

Tests should cover:

- URL normalization and canonicalization;
- SSRF policy and redirect revalidation;
- body/decompression/resource limits;
- parser determinism;
- candidate extraction;
- quality scoring and thresholds;
- IR serialization/canonical hashing;
- cache correctness;
- browser escalation without browser use on the ordinary path;
- property/fuzz tests for parsers and hostile input;
- shadow comparison metrics against the legacy pipeline.

Measure completeness, boilerplate, p50/p95 latency, outbound requests, browser launches, bytes, CPU, memory, and cost. A local site fix is not sufficient evidence for a global algorithm change.

## Cross-repository change rules

Use a workspace changeset when changing:

- Document IR or extraction events;
- public capture/result semantics;
- Knowledge ingestion expectations;
- browser-worker deployment/security requirements;
- blob ownership/reference contracts;
- migration or reprocessing behavior.

List affected producers/consumers, rollout order, compatibility, reprocessing needs, and rollback.

## Git and PR workflow

- Keep safety changes separate from scoring/performance changes when possible.
- Include fixtures or benchmark evidence for extraction behavior changes.
- State expected impact on fetch count, browser rate, latency, and quality.
- Do not add a new framework/provider without documenting which existing responsibility it replaces.
- Do not commit captured personal pages, credentials, cookies, or copyrighted full-site corpora as fixtures.
- Use minimized synthetic or permitted fixtures with clear provenance.
- Update documentation when a planned feature becomes implemented.

## Completion criteria

A task is complete only when:

- responsibility belongs to Extractor;
- the ordinary path preserves fetch-once/parse-once behavior;
- SSRF, redirects, sizes, timeouts, and resource budgets remain enforced;
- output is deterministic Document IR with provenance;
- no LLM or provider-account responsibility leaked into the service;
- browser escalation is justified, isolated, and bounded;
- golden-corpus and relevant security tests pass;
- telemetry explains selection and failure behavior without leaking content/secrets;
- contracts and reprocessing implications are documented;
- repository-local and workspace integration checks pass.
