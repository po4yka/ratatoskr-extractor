# Security Policy for Ratatoskr Extractor

> Status: Proposed  
> Last reviewed: 2026-08-17

Report vulnerabilities privately through GitHub private vulnerability reporting when enabled or another established private channel. Do not publish private URLs, session data, raw personal documents, credentials, or exploit payloads in public issues.

Security review is required for URL parsing, DNS/redirect logic, proxy settings, decompression, archive/PDF parsing, browser navigation, authenticated profiles, file paths, resource limits, sanitization, and renderer changes.

Baseline:

- Revalidate every redirect and resolved address against SSRF policy.
- Bound requests, redirects, bytes, decompressed size, DOM nodes, time, CPU, memory, and browser concurrency.
- Treat HTML, PDFs, headers, filenames, scripts, and renderer output as hostile.
- Isolate Chromium and keep provider credentials out of generic extraction.
- Never execute page content or trust extracted text as instructions.
