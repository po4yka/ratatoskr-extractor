## Why

YouTube is the largest single source class the extractor still processes through the generic HTML
path, which yields no transcript, no video metadata, and no deterministic text. The legacy monolith
detected every common YouTube URL form, pulled transcripts preferring manual subtitles, and
optionally archived 1080p video behind storage caps. Plan item 7 brings that capability into the
extractor as a bounded, typed, provenance-preserving adapter.

## What Changes

- Add a `youtube` extraction crate that maps every documented YouTube URL form (watch, shorts,
  live, embed, `/v/`, youtu.be, music.youtube.com, m.youtube.com, youtube-nocookie.com) to a video
  id and a canonical watch address, refusing non-video URLs into the ordinary HTML path.
- Add transcript retrieval: one fetch of the watch page, parse of the bounded embedded player
  response, explicit selection of a caption track (configured language order; manual preferred over
  auto-generated within a language), then one fetch of that track's timed-text document.
- Build Document IR from transcript segments: one paragraph per segment up to the block budget,
  merged deterministically past it. Video title, channel, duration, and selected language enter
  provenance and candidate metrics. Segment timing is preserved in extractor-owned storage keyed by
  block index plus the raw timed-text artifact; the shared block shape is unchanged (per the
  workspace `document-ir` spec, service-private data stays service-local).
- Add typed failure modes: no caption tracks at all, no track matching language preference, unparseable
  player response, oversized payloads - each with a distinct class and a defined terminal outcome.
- Add optional media archival behind an explicit config gate (default off): confined yt-dlp
  subprocess with wall-clock timeout and bounded output, hard per-item byte cap enforced while
  streaming into blob storage, a hard total byte budget accounted across runs, retention-based
  cleanup, and an artifact record carrying only BlobRef fields.
- Extend source classification so `youtube-nocookie.com` joins the documented YouTube hosts.
- Route `youtube`-classified runs through the adapter (`parser_version` `youtube-v1`) instead of the
  generic HTML path.

Out of scope: summarization (Knowledge), channel-level subscriptions/digests, audio transcription of
non-YouTube media, and any change to the shared document contracts repository.

## Capabilities

### New Capabilities

- `youtube-transcripts`: URL-form mapping to video ids, two-step bounded transcript retrieval with
  explicit track selection, transcript-to-Document-IR conversion with extractor-preserved timing,
  typed failure modes, and optional capped gated media archival with cleanup semantics.

### Modified Capabilities

- `safe-url-routing`: the documented-host list for the YouTube route gains `youtube-nocookie.com`
  and its embed/watch forms classify as YouTube rather than generic web.

## Impact

- New crate `crates/youtube` (`ratatoskr-extractor-youtube`) following the `pdf`/`providers`
  precedents for pure, offline-tested conversion with typed errors.
- `crates/url-routing`: classifier host list extension plus tests.
- `services/extractor`: a `complete_youtube` completion path dispatched on the `youtube`
  classification string, mirroring `complete_provider`'s typed degradation ladder.
- `crates/core`: new `[youtube]` config section (transcript languages; media gate, caps, timeout,
  binary path) with built-in defaults and validation rules.
- `schema.sql`: edited in place per development status - artifact kind gains `archived_media`; new
  `media_archives` bookkeeping table for budget accounting and retention cleanup.
- `crates/eventing`: `parser_version` branch for the YouTube route.
- No change to `ratatoskr-contracts`, event subjects, or payload shapes; the extracted event keeps
  its current shared schema.
