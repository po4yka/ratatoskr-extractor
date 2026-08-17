# Developing Ratatoskr Extractor

> Status: Proposed  
> Last reviewed: 2026-08-17

The repository is in architecture bootstrap. The extractor, browser worker, parsers, fixtures, and benchmarks are not implemented.

## Intended toolchain

Rust/Tokio, Reqwest/Rustls, html5ever/scraper, PDF adapters, bounded CPU work, SQLx/PostgreSQL, BlobStore, Chromium behind a `BrowserRenderer`, tracing, testcontainers, fuzz/property tests, and benchmark tooling.

## Workflow

1. Classify the source and preserve the original URL.
2. Change one stage without bypassing shared fetch, parse, quality, or provenance logic.
3. Add or update a licensed/synthetic golden fixture.
4. Test SSRF, redirects, limits, malformed input, determinism, and cancellation.
5. Compare quality, latency, requests, bytes, CPU, memory, and browser escalation against baseline.

The first scaffold PR must document exact build, format, lint, unit, integration, fuzz, corpus, benchmark, browser, and migration commands. LLM credentials must never be needed because LLM interpretation is outside this repository.
