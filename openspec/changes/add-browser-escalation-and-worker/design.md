## Context

The stored `browser-rendering` capability defines the wire behaviour. This change implements both
sides inside this repository: the browser-worker deployable consumes commands and publishes
evidence; the extractor escalates deterministically and re-parses results through the existing
HTML path. Chromium is reached through `chromiumoxide 0.9` (pure Rust CDP client, MIT/Apache-2.0,
actively maintained, small dependency surface: tungstenite + generated CDP types).

## Goals / Non-Goals

Goals: contract-faithful job types; one navigation per fresh context; default-denied heavy
subresources; downloads off; worker-side SSRF revalidation on target and hops; finite budgets;
owned BlobRef evidence; idempotent completion; deterministic bounded escalation; same parser and
evaluator for rendered DOM.

Non-Goals: credential surfaces of any kind; stealth or anti-bot bypass; screenshots, PDF printing;
OCR; multi-tab rendering; a blob service; changing any direct-extraction byte when escalation does
not trigger.

## Decisions

### One crate for the schema, one binary for the worker

`crates/render-job` holds serde types with `deny_unknown_fields` on commands (a caller cannot
smuggle cookie or storage fields the schema lacks — this is the credential-denial mechanism at the
type level) and tolerant-but-typed event payloads. Golden tests pin the serialized shapes so drift
against the stored contract surfaces in review.

### Browser lifecycle: launch once per process, isolate per job

The worker starts Chromium lazily on the first job and reuses the process; each job opens a new
incognito context and closes it in all paths (including cancellation) via structured teardown.
This matches the stored isolation requirement while avoiding per-job process startup cost.

### Request interception through CDP Fetch domain

`Fetch.enable` with patterns denying `Image`, `Font`, `Media`, and `WebSocket` resource types and
allowing Document/Script/Stylesheet/Fetch/XHR; denied counts accumulate into the evidence summary.
Downloads are disabled by page configuration.

### SSRF revalidation reuses `url-routing`

The worker validates the command URL before navigation with `validate_address` plus port policy,
and revalidates each redirect hop reported by CDP network events before the browser follows it;
refusal aborts the job as `policy_blocked`. This keeps one policy implementation shared by both
deployables instead of a divergent copy.

### Budgets map to CDP timeouts plus an overall tokio deadline

Navigation budget becomes `goto`'s timeout; total budget wraps the whole job in a deadline that
also drives teardown; size cap truncates capture only by failing (`size_limit`) rather than
publishing partial DOM.

### Escalation trigger and bounds

After `reject_quality`, the extractor inspects the raw HTML for empty-shell evidence: hydration
mount markers (`id="root"`, `id="app"`, `__NEXT_DATA__`) together with near-zero extracted text.
If present and `render.enabled`, it publishes one render command derived from the run identity,
renews the run lease every few seconds while subscribed to completion/failure for its `render_id`
(bounded by the render total budget), then either re-parses the rendered blob through `from_html`
with provenance naming the rendered artifact or terminates with the carried class. The escalated
parse cannot escalate again: the code path has no escalation branch.

### Deduplication through JetStream KV

A KV bucket keyed by `render_id` marks completed jobs before the completion event publishes;
redelivered commands for marked ids are acknowledged without work, satisfying the stored
at-least-once clause across worker restarts within bucket TTL.

## Risks / Trade-offs

[Chromium availability] → integration tests require `CHROME_BIN`; documented beside PostgreSQL/
JetStream prerequisites and installed in CI, keeping skipped-test rules intact.
[CDP API churn] → chromiumoxide pins generated CDP types per release; upgrades are explicit.
[Escalation storms] → per-host caps and single-command-per-run structure bound cost; strategy
tables stay future work.
[Shared volume trust] → extractor reads worker-owned blobs by digest verification before parsing;
mismatched bytes fail as artifact errors.

## Migration Plan

Worker deploys first behind its compose profile with no producer; extractor escalation ships after
and stays inert until `render.enabled`. Rollback disables the flag; no persisted state changes.

## Open Questions

None blocking; host-strategy tables belong to future plan work.
