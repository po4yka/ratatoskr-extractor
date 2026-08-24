## Why

Provider-classified Hacker News and Reddit runs currently convert only self-contained post bodies. A link post — the majority of HN submissions — carries its payload in an external article URL, and such runs today end as low-quality conversions because the adapter ignores the stored link. When the native JSON fetch or schema conversion fails after classification, the run terminates instead of trying the generic HTML path, and request pacing exists only as global and per-host concurrency caps.

## What Changes

- Provider adapters gain the resolution half: after the single native-JSON fetch, a link post resolves to its canonical external article URL and the run continues through exactly one ordinary retrieval/extraction pass on that target; a self-text post keeps building Document IR directly from the JSON.
- The resolved target re-enters the ordinary pipeline unchanged — full SSRF policy, redirect revalidation, size limits, parsing, candidates, and quality scoring apply to it like any other URL.
- Provider failure becomes graceful: when the provider fetch or schema conversion fails, the run records the typed provider failure class and makes exactly one generic HTML attempt on the original URL instead of terminating.
- Every resolution step — provider attempt outcome, resolved target, fall-through decision — is persisted as an extractor-owned fact tied to the run.
- Safe fetch gains per-host request-rate limiting alongside the existing concurrency admission.
- Schema change edits `schema.sql` in place, adding a resolution-steps record keyed by run (no migration ledger while Ratatoskr is in development).

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `provider-adapters`: link posts resolve to their canonical article target and continue once through the ordinary path; self-text conversion is unchanged; provider fetch/schema failures fall through to exactly one generic HTML attempt instead of failing the run; the resolution chain is recorded on the run.
- `event-pipeline`: terminal transactions also commit resolution-step records among the extractor-owned facts.
- `safe-fetch`: adds a per-host request-rate limit next to the finite operation budget and concurrency admission controls.

## Impact

- Code: `crates/providers` (link/self-text resolution mapping), `services/extractor/src/pipeline.rs` (single-hop continuation and fall-through), `crates/eventing` (resolution record writes), `crates/safe-fetch` (host pacing), `crates/core/src/config.rs` (rate-limit settings), `schema.sql` (resolution table), provider fixtures and tests.
- Contracts: none cross-repository. Document IR shape and bus subjects are unchanged; a resolved document keeps the original capture identity while its recorded final URL follows the existing provenance rules.
- Docs: README status wording and `docs/IMPLEMENTATION_PLAN.md` item annotations updated to describe the resolution half.
