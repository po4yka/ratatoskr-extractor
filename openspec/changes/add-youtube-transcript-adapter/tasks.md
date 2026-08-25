## 1. Classification: youtube-nocookie.com

- [x] 1.1 Add failing tests in `crates/url-routing/tests/classification.rs`: `www.youtube-nocookie.com/embed/<id>` classifies `YouTube`; `youtube-nocookie.com.example.test/watch?v=<id>` stays `GenericWeb`. Run and record the red output.
- [x] 1.2 Extend the classifier host list (`crates/url-routing/src/lib.rs`) so both tests pass; run the full url-routing suite green.

## 2. Youtube crate scaffold and URL identity

- [x] 2.1 Scaffold `crates/youtube` (`ratatoskr-extractor-youtube`, lib `extractor_youtube`) with workspace lints, deps (document-ir contracts, serde, thiserror, sha2), registered in the root workspace. No failing test: scaffolding and manifest wiring only; verified by `cargo check -p ratatoskr-extractor-youtube`.
- [x] 2.2 Add failing mapping tests `crates/youtube/tests/mapping.rs`: watch `?v=`, `/shorts/`, `/live/`, `/embed/`, `/v/`, `youtu.be/<id>`, `music.youtube.com`, `m.youtube.com`, `youtube-nocookie.com` forms resolve to the same id and canonical watch address; share params excluded from canonical form; playlist-only and malformed-id inputs yield the typed fall-through error. Record red output.
- [x] 2.3 Implement identity extraction and canonicalization until `cargo test -p ratatoskr-extractor-youtube` is green.

## 3. Player response parsing

- [x] 3.1 Add fixtures under `crates/youtube/tests/fixtures/` (synthetic, first-party text): watch page with embedded player response carrying manual + auto tracks, page without player data, brace-hostile page, oversized padded page. Add failing parser tests: title/channel/duration extracted; zero-track and malformed pages typed errors; oversized input resource-limited; no panics on hostile bytes. Record red output.
- [x] 3.2 Implement bounded marker-plus-brace extraction and serde parsing until those tests are green.

## 4. Track selection

- [x] 4.1 Add failing selection tests: manual preferred within top language; language order dominates manual-in-lower-language; generated-only top language accepted; zero tracks -> `NoTranscript`; none matching preference -> `NoLanguageMatch`. Record red output.
- [x] 4.2 Implement selection against configured ordered languages until green.

## 5. Timed-text parsing

- [x] 5.1 Add XML and JSON3 timed-text fixtures plus failing parser tests: segments parsed in order with ms timing; segment-count, byte, and per-segment text budgets enforced as typed limits; unrecognized payload typed schema error; double-call determinism. Record red output.
- [x] 5.2 Implement both bounded parsers until green.

## 6. Document IR conversion and timing sidecar

- [x] 6.1 Add failing conversion tests: one paragraph per segment under budget; deterministic even merge past budget; identical fixtures produce equal documents and digests twice; strategy `youtube_transcript`; document language equals selected track language; candidate metrics carry title/channel/duration/language/segment count; sidecar ranges map every block index to ms bounds and match block count. Record red output.
- [x] 6.2 Implement conversion through `assemble_document` plus sidecar construction until green.

## 7. Configuration

- [x] 7.1 Add failing config tests: `[youtube]` defaults (languages `["en"]`, media gate off, caps 2 GiB item / 8 GiB total, retention 24h, timeout 900s, height 1080); validation rejects empty languages, zero caps, total < item, retention < 1h, timeout < 1s with value-free violations; env override round-trips. Record red output.
- [x] 7.2 Implement the config section, built-ins, and validation until green.

## 8. Schema bookkeeping

- [x] 8.1 Edit `schema.sql` in place: artifact kind gains `archived_media`; add `media_archives` table (run reference, video id, digest, length, created/expires) with bounded-value checks. No failing test: declarative schema edited under development status; verified by the persistence schema integration test recreating a database from the definition.

## 9. Archival orchestration

- [x] 9.1 Define the `MediaDownloader` trait seam and fakes. Add failing orchestrator tests with an injected oversized fake (per-item cap aborts mid-stream, no row/blob remains, run outcome unaffected), exhausted-total-budget fake (skip recorded), duplicate video id skip while unexpired, downloader failure recorded but success preserved, gate disabled performs no download call. Record red output.
- [x] 9.2 Implement budget accounting (advisory-lock transaction over unexpired sums), streaming copy with mid-copy cap into blob store, purge of expired rows/bytes including orphan sweep, until green.

## 10. Confined yt-dlp runner

- [x] 10.1 Add a subprocess test using a local fixture script standing in for the binary (offline): argv contract asserted, wall-clock timeout kills a sleeping process, stderr capture capped, nonzero exit maps to the archival failure class. Record red output.
- [x] 10.2 Implement the production `MediaDownloader` over `tokio::process` with the confinement rules from design D6 until green.

## 11. Service integration

- [x] 11.1 Add failing end-to-end service test (scripted loopback server serving watch-page and timed-text fixtures; Postgres/JetStream from compose): `youtube`-classified run succeeds with exactly two requests, published document, raw artifacts, diagnostics sidecar, `parser_version` `youtube-v1`; a no-tracks fixture ends rejected quality with class `youtube_no_transcript`. Record red output.
- [x] 11.2 Implement `complete_youtube`, dispatch match, and `parser_version` branch until the service test and existing provider/pdf suites stay green.

## 12. Documentation and gate

- [x] 12.1 Update README status/source-classification sections and `docs/INTERFACES.md` SourceAdapter mention; note the deferred contracts change for boundary-visible timing. No failing test: documentation only; verified by review diff.
- [x] 12.2 Run the full gate from DEVELOPMENT.md (compose postgres+nats up, Chrome available): fetch/deny/fmt/clippy/build/test/test-doc/release build, file-size ratchet, `openspec validate --all --strict`, and tick tasks only after green evidence.
