## Why

The rendering contract is now defined in the workspace store (`add-browser-rendering-contract`),
and plan item 8 calls for the isolated browser-worker deployable plus strict escalation wiring in
the extractor. Today every hydration-required page terminates through generic HTML quality gates
with no path to rendered content.

## What Changes

- Add `crates/render-job`: serde types implementing the stored contract for render commands,
  completion events, failure events, and their evidence summary, with golden serialization tests.
- Add the second deployable `services/browser-worker` (binary `ratatoskr-browser-worker`): durable
  JetStream command consumption, per-job fresh-context Chromium rendering through `chromiumoxide`,
  default-denied heavy subresources, downloads disabled, worker-side SSRF revalidation of the
  target and every redirect hop through `url-routing`, navigation/total/size budgets, owned
  BlobStore persistence of rendered DOM, completion/failure publication, and KV-bucket
  `render_id` deduplication.
- Wire deterministic escalation into the extractor: after an HTML low-quality rejection whose
  evidence matches an empty-shell shape, publish one bounded render request, keep the run lease
  alive while awaiting the result, re-parse returned DOM through the ordinary HTML path with
  provenance naming the rendered artifact, and map worker failure classes to terminal classes.
- Add a `browser` compose profile running the worker against a Chromium image with memory and PID
  limits, and document the new test-environment requirement (`CHROME_BIN`) beside the existing
  PostgreSQL/JetStream ones.
- Keep OCR, screenshots, PDF printing, credential surfaces, and stealth capabilities outside this
  change.

## Capabilities

### New Capabilities

- `render-pipeline`: implementation-side behaviour binding this repository to the stored
  `browser-rendering` contract — job schema stability, worker isolation and budget enforcement,
  escalation determinism, and terminal mapping.

### Modified Capabilities

- `event-pipeline`: escalated runs renew their lease while awaiting a result and terminate with
  carried render failure classes instead of generic quality rejection when the worker fails.

## Impact

New crate and new binary join the workspace, the lockfile, deny checks, and the CI gate list;
`compose.yaml` gains a profiled service; `DEVELOPMENT.md` and `.github/workflows/ci.yml` gain the
Chrome test prerequisite together or not at all. Direct extraction paths stay byte-for-byte
identical when no escalation triggers.
