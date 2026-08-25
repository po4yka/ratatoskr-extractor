## Context

The extractor already classifies YouTube hosts (`SourceRoute::YouTube`, persisted as
classification `"youtube"`) but dispatches such runs down the generic HTML path
(`services/extractor/src/pipeline.rs` maps only `"hacker_news"` and `"reddit"` to provider
routes). Provider adapters assume exactly one JSON fetch followed by pure conversion; PDF shows
the precedent for a format-specific crate with its own completion path and config section. The
shared `DocumentBlock` remains `Heading | Paragraph` with `deny_unknown_fields`, and the workspace
`document-ir` spec refuses service-private fields in the shared shape, so transcript timing cannot
ride blocks across the boundary. The extractor depends on `ratatoskr-document-contracts` through a
pinned git rev of the contracts repository, which this change does not modify. Development status
allows editing `schema.sql` in place with no migration tooling.

## Goals / Non-Goals

**Goals:**

- Every documented YouTube URL form yields one deterministic video identity before any network work.
- Transcript retrieval stays inside the existing safe-fetch policy (SSRF, redirects, size,
  timeouts) with each request individually justified and recorded.
- Typed, distinguishable failure modes with fixed terminal outcomes and no automatic retries.
- Optional media archival that can never degrade an accepted extraction and can never exceed its
  byte budgets.
- Fully offline test suite: recorded fixtures, scripted loopback server, injected fake downloader.

**Non-Goals:**

- Any change to `ratatoskr-contracts`, event subjects, or payload shapes.
- Summarization, channel digests, audio transcription of non-YouTube media.
- Anti-bot bypass; a challenge page is a typed degradation, not an obstacle to defeat.
- A general-purpose yt-dlp wrapper feature surface beyond this adapter's needs.

## Decisions

### D1: New crate `crates/youtube` beside `pdf`/`providers`

The transcript flow needs two dependent fetches (watch page, then the timed-text document named
inside it), which does not fit `complete_provider`'s single-response contract. Following the PDF
precedent keeps conversion pure and offline-tested in `ratatoskr-extractor-youtube`
(lib `extractor_youtube`) while `services/extractor` owns a small `complete_youtube` choreography
keyed on the classification string, mirroring `complete_pdf`'s structure and typed error ladder.
Alternative considered: growing `crates/providers` - rejected because its `SourceRoute`,
limits type, and one-fetch entry point would all need breaking reshaping for one route.

### D2: Watch page + embedded player response instead of the InnerTube API

Fetch the canonical watch URL as HTML and extract the bounded `ytInitialPlayerResponse` JSON
(marker find plus brace matching under a hard byte budget, then serde). This matches the legacy
approach, uses the retriever's existing HTML path, and avoids InnerTube client-context spoofing and
POST plumbing. Video title, channel, duration, and caption tracks come from that structure.
Alternative considered: `youtubei/v1/player` POST - rejected for now (new request shape in
safe-fetch, fragile client versioning) and revisitable without spec changes because track data is
schema-tolerant.

### D3: Timing is preserved extractor-owned; shared blocks stay untouched

Per the workspace `document-ir` spec ("a service-private field does not enter the shared shape"),
segment timing lives extractor-side: the raw timed-text response is retained as a `raw_source`
artifact, and a `diagnostics` artifact records, for each produced block index, the covered segment
range (`start_ms`, `end_ms`). Blocks map one-to-one to segments until the block budget; past it,
consecutive segments merge deterministically (even split over cue boundaries) and the sidecar
stores merged ranges, so fidelity survives grouping. Reprocessing needs no network. If Knowledge
ever needs timing at the boundary, that is a coordinated contracts changeset in the contracts
repository - explicitly out of scope here.

### D4: Track selection order is language rank first, manual-over-generated second

Configured languages iterate in order; within one language a manually authored track beats an
auto-generated one; the first match wins. Wrong-language manual text scores worse than right-language
generated text, so language dominates. The selected track's language becomes the document language;
selection failures are typed (`youtube_no_transcript` when zero tracks exist, distinct
`youtube_no_language_match` when tracks exist but none matches).

### D5: Timed-text parsing accepts both wire formats

The timed-text endpoint returns either XML (`<text start="" dur="">`) or JSON3 (`events`). Both
parsers are bounded (byte budget, segment count, per-segment text length), deterministic, and
fixture-tested; an unrecognized payload is a typed schema failure that degrades like any other
parse failure rather than guessing.

### D6: Media downloads go through a confined yt-dlp subprocess behind a trait

There is no maintained Rust crate that reproduces yt-dlp's DASH handling, and transcripts do not
need yt-dlp at all - so yt-dlp is confined to the optional archival path behind a
`MediaDownloader` trait with two implementations: a production subprocess runner and test fakes.
Confinement rules: resolved absolute binary path from config, argv-only invocation (no shell),
fixed argument template including height cap and no-playlist, dedicated temporary working
directory, hard wall-clock timeout with process-tree kill, stderr captured under a byte cap, empty
scrubbed environment, and no credentials ever passed. Bytes stream from the produced file into the
extractor-owned blob store with the SHA-256 computed during the copy and the per-item cap enforced
mid-copy; oversized or failed attempts delete their partial state before returning. Tests inject
fakes (including an oversized producer) so caps and cleanup are exercised without the binary or
network.

### D7: Budget accounting lives in a new `media_archives` table under an advisory lock

`schema.sql` gains an `archived_media` artifact kind and a `media_archives` bookkeeping table
(`run_id`, `video_id`, digest, `length_bytes`, `created_at`, `expires_at`). The total-budget check
(`sum(length_bytes)` over unexpired rows) and the insert run in one transaction guarded by a
PostgreSQL advisory lock, serializing concurrent archives; the residual multi-writer risk is bounded
by the lock being taken before any download completes. An unexpired record for the same `video_id`
skips re-download (idempotent replay). Purge deletes expired rows and their stored bytes; blob
removal precedes row removal so a crash leaves at worst a content-addressed orphan file, which the
purge sweep also collects. Artifact rows for purged media are removed with the bookkeeping row.

### D8: Configuration follows the `render.enabled` gate model

A `[youtube]` section in `ExtractorConfig`: `transcript.languages` (default `["en"]`),
`media.enabled` (default `false`), `media.max_item_bytes` (2 GiB), `media.total_budget_bytes`
(8 GiB), `media.retention_hours` (24), `media.timeout_secs` (900), `media.max_height` (1080),
`media.binary_path` (`yt-dlp`). Validation rejects empty language lists, non-positive caps,
total < item, retention < one hour, timeout < one second - value-free violations as elsewhere.

### D9: Dispatch and degradation ladder

`parser_version("youtube")` becomes `"youtube-v1"`; the pipeline match routes `"youtube"` to
`complete_youtube`. Ladder: identity resolution failure -> one ordinary HTML attempt (class
`youtube_no_video_id`); missing/unparseable player response -> one ordinary HTML attempt (class
`youtube_player_schema`); zero caption tracks -> reject quality `youtube_no_transcript`; no
language match -> reject quality `youtube_no_language_match`; budget exceeded -> fail run as parse
resource limit; successful conversion -> the standard document completion path plus best-effort
archival when gated on. Archival outcomes (stored, skipped-duplicate, skipped-budget, failed-class)
are recorded in candidate metrics and diagnostics, never in the run status.

## Risks / Trade-offs

- [YouTube markup/format drift breaks player-response or timed-text parsing] -> Tolerant bounded
  schemas, typed schema failures with one generic-HTML degradation, fixture corpus pinned in-repo;
  strategy name and parser version recorded for reprocessing.
- [Embedded JSON extraction mis-scopes braces on hostile pages] -> Hard scan budget, strict
  marker anchoring, serde `deny_unknown_fields` on required nodes, fuzz-style pathological
  fixtures.
- [Timed-text location points off-host] -> HTTPS-plus-YouTube-host validation before the second
  fetch; anything else is treated as no usable transcript and never requested.
- [Total-budget race across service replicas] -> Advisory lock serializes check-and-insert; worst
  residual overshoot is one in-flight archive whose bytes were already capped per item.
- [yt-dlp exit-code and flag drift across versions] -> Uniform nonzero/timeout handling as a
  single archival failure class; binary path and height cap configurable; archival is never fatal.
- [Long videos exceed the block budget] -> Deterministic merge rule with sidecar ranges preserves
  recoverable timing; budget configurable through existing limits patterns.

## Migration Plan

Ship with `media.enabled` false; transcript extraction activates for `youtube` classification on
deploy exactly as the provider adapters did. Test databases rebuild from the edited `schema.sql`;
no data preservation exists or is needed under development status. Rollback is configuration-only
(route dispatch reverts by reverting the release). Archival activation is a per-deployment
configuration act after storage owners confirm the retention policy.

## Open Questions

None blocking. Whether archived-media BlobRefs should be announced cross-service through
`evt.platform.operation.reported.v1` payloads is a workspace event-contract question deferred until
Vault/storage owners define consumption.
