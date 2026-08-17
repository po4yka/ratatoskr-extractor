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
