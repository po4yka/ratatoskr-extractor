# Extractor threat model

## Assets

Internal network, filesystem, CPU/memory, browser profiles, private captures, BlobStore integrity, and trusted downstream consumers.

## Threats and controls

- **SSRF/DNS rebinding/redirect pivot:** canonical parsing, address policy, resolution pinning where appropriate, and validation every hop.
- **Response/decompression bomb:** streaming limits and early cancellation.
- **Malformed HTML/PDF/parser exploit:** patched parsers, fuzzing, sandboxing where available, and process isolation for high-risk formats.
- **Browser escape/resource exhaustion:** isolated unprivileged worker, context per job, blocked unnecessary resources, quotas, timeout, kill group.
- **Prompt/content injection:** extracted text remains untrusted data; no instruction execution.
- **Path/filename traversal:** content-addressed internal names and validated temporary paths.
- **Cache poisoning/cross-user leak:** owner/policy-aware keys and authorization before retrieval.
- **Telemetry leak:** hash/host-safe metrics, bounded labels, no body/query/header secrets.

Re-review for authenticated rendering, new file formats, proxies, local-file support, JavaScript hooks, or remote extraction providers.
