# Proposal: harden-browser-escalation-policy

## Why

The isolated browser worker shipped behind a single content-shape trigger (`render.enabled` plus empty-shell evidence), so any hydration shell on any host can spend Chromium cost once the flag flips on, and nothing bounds how often that happens per day. The plan item this repository tracks calls for an explicit escalation policy - static attempt degraded AND host matches configured policy AND budget permits - with the policy's gate combinations pinned by tests, and for a worker that recycles itself before leaks accumulate.

## What Changes

- Extractor escalation decisions move from inline booleans in the pipeline into one deterministic, pure policy evaluation whose inputs are the quality rejection, empty-shell evidence, the master switch, the target host against a configured allowlist, the remaining per-day budget, and the render budgets. Every gate combination is covered by unit tests, and the policy denies unless every gate permits.
- `RenderConfig` gains `allowed_hosts` (exact-host match, case-insensitive; empty list means no host restriction beyond the other gates) and `max_escalations_per_day` (finite by default).
- A durable per-UTC-day counter in the extractor schema records each published render command; the check-and-increment happens in one transaction so concurrent runs cannot exceed the budget. Schema changes edit `schema.sql` in place per the development status.
- The browser worker exits cleanly after a configured finite number of handled jobs per process (`BROWSER_MAX_JOBS_PER_PROCESS`, finite by default), letting the supervisor restart it with fresh Chromium; compose/systemd restart policy and cgroup memory/PID limits remain the leak backstop.
- The production worker binary launches the real Chromium executor (never a refusal-only placeholder) from typed `BROWSER_CHROME_BIN` configuration, preserves the default SSRF denial for loopback, and its compose profile declares CPU, memory, PID, and restart limits so process recycling is actually supervised.
- Stale phase text in `AGENTS.md` (still claiming the browser worker is absent) is corrected to match the implemented state.

Out of scope, unchanged from the delivered design: LLM-driven browsing, authenticated sessions or cookies, captcha solving, stealth/anti-bot bypass, distributed fleets.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `render-pipeline`: escalation becomes a gated policy decision (host allowlist, per-day budget, recorded denial) instead of shape-evidence alone, and the worker gains a finite per-process job budget with clean exit.

## Impact

- `services/extractor/src/pipeline.rs` and a new `escalation` module own the decision function; `crates/core/src/config.rs` carries the two new validated fields; `schema.sql` gains one accounting table; `crates/persistence` and `crates/eventing` gain the counter call used before publishing the command.
- `services/browser-worker/src/lib.rs` counts handled jobs and returns cleanly at the limit; deployment examples carry the new environment knob.
- `services/browser-worker/src/main.rs` owns real-executor startup, while `compose.yaml` supplies the production process ceilings and restart policy.
- Tests: policy unit matrix, schema integration test, extractor integration denials with zero published commands, worker transport exit test. Existing golden wire shapes, dedup, SSRF, and budget behaviour do not change.
