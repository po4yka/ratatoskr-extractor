## Purpose

Turns classified YouTube URLs into deterministic transcript-based Document IR with preserved
timing evidence and an optional, strictly capped media archive, replacing generic HTML extraction
for every documented YouTube URL form.

## ADDED Requirements

### Requirement: Every documented URL form resolves to one video identity

The adapter SHALL resolve the watch (`?v=`), `/shorts/`, `/live/`, `/embed/`, and `/v/` path forms,
`youtu.be` short links, the `music.youtube.com` and `m.youtube.com` hosts, and
`youtube-nocookie.com` embed and watch forms to the same eleven-character video identity and one
canonical watch address. Share-attribution parameters other than the video identity SHALL be
excluded from the canonical address while the original URL remains preserved upstream. A YouTube
URL that does not resolve to a video identity (for example a playlist-only URL) SHALL NOT take the
transcript path.

#### Scenario: Shorts and watch forms agree

- **WHEN** a `/shorts/<id>` URL and a `watch?v=<id>` URL carry the same id
- **THEN** both resolve to that id and to the same canonical watch address

#### Scenario: A playlist-only URL falls through

- **WHEN** a `youtube.com` URL names no video identity
- **THEN** the run takes the ordinary HTML path unchanged and no transcript fetch occurs

#### Scenario: A malformed identity is refused

- **WHEN** an extracted candidate id does not match the eleven-character id alphabet
- **THEN** the resolution fails with a typed error and the run falls back to the ordinary HTML path

### Requirement: Transcript retrieval performs exactly two justified fetches

For a resolved video identity the adapter SHALL make exactly one fetch of the watch page through
the ordinary safe-fetch policy and, when a caption track is selected, exactly one further fetch of
that track's timed-text document. The second fetch is justified by protocol correctness: the
timed-text location exists only inside the fetched page. The track location SHALL be fetched only
when it is an HTTPS URL on a documented YouTube host; any other location SHALL be treated as no
usable transcript and never requested.

#### Scenario: The full flow makes two requests

- **WHEN** a run completes from a served watch page and its selected timed-text document
- **THEN** exactly two requests were made, both through safe-fetch policy, and both payloads are
  retained as raw artifacts

#### Scenario: A foreign track location is never requested

- **WHEN** the embedded player response advertises a timed-text location on a non-YouTube host
- **THEN** the run records a typed no-transcript outcome and no request is made to that location

### Requirement: Track selection is explicit and prefers manual subtitles

Caption-track selection SHALL iterate the configured language preference in order and, within one
language, prefer a manually authored track over an auto-generated one. The first track under that
order wins. When no advertised track matches the preference the run SHALL end in a typed
no-matching-language failure distinct from a video having no tracks at all.

#### Scenario: Manual beats generated within the top language

- **WHEN** the highest-preference language offers both a manual and an auto-generated track
- **THEN** the manual track is selected

#### Scenario: Language order dominates

- **WHEN** the top-preference language offers only an auto-generated track and a lower-preference
  language offers a manual track
- **THEN** the top-preference auto-generated track is selected

#### Scenario: No track matches the preference

- **WHEN** no advertised track's language appears in the configured preference list
- **THEN** the run ends in a typed no-language-match failure naming no track contents

### Requirement: Transcript conversion is bounded and deterministic

Transcript conversion SHALL produce Document IR through the shared constructor with strategy
`youtube_transcript`, one paragraph per segment in segment order up to the block budget, and a
deterministic merge of consecutive segments past that budget. Identical input bytes SHALL produce
identical blocks, provenance, and content digest. Video title, channel name, duration, and the
selected track language SHALL be recorded in candidate metrics, and the document language SHALL be
the selected track language. Source-byte and produced-block budgets SHALL bound every stage.

#### Scenario: Identical fixtures convert identically

- **WHEN** the same watch page and timed-text fixtures are converted twice
- **THEN** both conversions yield equal documents with equal content digests and one selected
  candidate

#### Scenario: An oversized page is bounded

- **WHEN** the watch-page payload exceeds the configured input budget
- **THEN** conversion stops with a typed resource-limit failure rather than truncating silently

### Requirement: Segment timing stays extractor-owned

The published Document SHALL carry transcript text as plain paragraphs with the unchanged shared
block shape. Per-block timing SHALL be preserved in extractor-owned storage: the raw timed-text
artifact SHALL be retained, and a diagnostics artifact SHALL record for every produced block the
segment time range it covers, so the mapping is recoverable without the network. Reprocessing from
those artifacts SHALL require no new request.

#### Scenario: Timing survives without entering the shared shape

- **WHEN** a run succeeds and publishes its document
- **THEN** the published blocks contain no timing fields, and the run's stored diagnostics map each
  block index to a start-and-duration range in milliseconds

### Requirement: Transcript failure modes are typed and terminal outcomes fixed

A watch page with no embedded player response data, an unparseable player response, or an
anti-bot-shaped page SHALL degrade to exactly one ordinary HTML retrieval attempt with a recorded
failure class. A video whose player response advertises no caption tracks SHALL end as a rejected
quality outcome with class `youtube_no_transcript`; a missing language match SHALL end with class
`youtube_no_language_match`. Exceeding a size budget SHALL fail the run as a parse resource limit.
None of these outcomes SHALL retry the transcript flow automatically.

#### Scenario: A shell page degrades once

- **WHEN** the fetched watch page carries no usable player response
- **THEN** the run attempts ordinary HTML extraction exactly once more and records the degradation
  class

#### Scenario: A video without tracks is rejected, not failed

- **WHEN** the player response parses but advertises zero caption tracks
- **THEN** the run terminates as rejected quality with class `youtube_no_transcript`

### Requirement: Media archival is gated, capped, and never fatal

Video media SHALL be downloaded only when the explicit configuration gate is enabled, and only
after the transcript document has been accepted. Acquisition SHALL enforce a hard per-item byte
cap during streaming so an oversized item is aborted and its partial data discarded, and a hard
total budget computed from persisted accounting so an exhausted budget skips acquisition with a
recorded reason. An archival failure of any kind SHALL leave a successful extraction successful.
The stored artifact record SHALL carry BlobRef fields only - owner, digest, effective media type,
and length - and never a filesystem path.

#### Scenario: The gate is off by default

- **WHEN** configuration does not enable media archival
- **THEN** no download process is ever started and runs behave exactly as without the feature

#### Scenario: An oversized item is aborted at the cap

- **WHEN** acquired media exceeds the per-item cap mid-stream
- **THEN** acquisition aborts, no artifact or bookkeeping row remains, and the run still succeeds

#### Scenario: An exhausted total budget skips archival

- **WHEN** persisted accounting shows the total budget consumed
- **THEN** acquisition is skipped with a recorded budget reason and the run still succeeds

#### Scenario: Archival failure does not fail the run

- **WHEN** the download process times out or exits nonzero
- **THEN** the run keeps its succeeded extraction outcome and stores the archival failure class as a
  diagnostic

### Requirement: Archived media expires and frees its budget

Each archived-media record SHALL carry a retention deadline. Purge SHALL delete the stored bytes
and the bookkeeping row for every record past its deadline, including records left by crashed
runs, and purged records SHALL immediately stop counting against the total budget.

#### Scenario: Expired media is purged and its budget freed

- **WHEN** purge runs with a record past its retention deadline
- **THEN** the stored bytes and the row are gone and the total-budget accounting reflects the
  removal
