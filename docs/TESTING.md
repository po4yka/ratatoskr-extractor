# Extractor testing strategy

## Test layers

- URL normalization/classification properties and adversarial SSRF cases.
- Redirect, DNS, timeout, body, decompression, MIME, and cache behavior with local servers.
- DOM candidate determinism, quality scoring, provenance, and IR round trips.
- HTML/PDF fuzzing and malformed fixtures.
- Browser isolation, blocked resources, cancellation, memory/time limits, and no profile leakage.
- SQL migrations, outbox/inbox replay, BlobStore verification, and duplicate commands.
- Golden corpus for static, malformed, multilingual, code/table-heavy, SPA, paywall, PDF, and error pages.

## Regression gates

Track completeness/boilerplate, p50/p95 latency, requests and bytes per URL, CPU, memory, browser escalation, and failure class. A quality or cost regression requires explicit approval and fixture evidence.

Fixtures must be synthetic, licensed, or captured with permission and scrubbed of credentials/personal data.

## Test-first

A change is planned before it is built, and the plan is a task list in which behaviour arrives in
pairs: one task adds a failing test, the next makes it pass. `openspec/config.yaml` carries that
rule, which is what puts it into every planning and implementation request rather than only into this
document.

The loop:

1. Write the test the scenario names. Run it. Confirm it fails, and read the failure — a test that
   fails because it does not compile has proved nothing about the behaviour.
2. Write the smallest change that makes it pass. Run it again.
3. Refactor only once it is green, adding no test and changing no behaviour.

Two checks stand behind this, and neither of them can see the order:

- `openspec validate --archived`, in `.github/workflows/openspec.yml`, fails when a change was
  archived with a task left unticked.
- A step in `.github/workflows/fleet.yml` fails when this repository holds a manifest and a `ci.yml`
  that never runs a test.

`ratatoskr-workspace/docs/QUALITY_GATES.md` records why the order itself is not checkable, rather
than leaving the gap to be discovered.
