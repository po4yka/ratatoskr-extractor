# Extractor requirements

> Status: Proposed  
> Last reviewed: 2026-08-17

## Goals

1. Convert supported URLs/files into deterministic, provenance-rich Document IR.
2. Fetch once and parse once on the normal HTML path.
3. Route provider-native sources before generic scraping.
4. Explain candidate selection and browser escalation.
5. Enforce network, parser, browser, and resource safety.

## Non-goals

Summaries, embeddings, LLM navigation/interpretation, provider-account synchronization, and hidden session scraping.

## Requirements

- Normalize and classify while preserving original/canonical/redirect history.
- SSRF validation applies before connection and after every redirect/DNS resolution.
- HTTP, decompression, DOM, PDF, browser, and artifact sizes are bounded.
- Multiple extractors consume one parsed DOM and produce scored candidates.
- Accepted output includes IR, metadata, provenance, content hash, diagnostics, and strategy versions.
- Browser use is exceptional, isolated, bounded, observable, and uses the same extraction/scoring path.
- Reprocessing unchanged content is idempotent.

First slice: static HTML URL -> safe fetch -> one DOM -> at least two candidates -> quality decision -> Document IR -> artifact/event.
