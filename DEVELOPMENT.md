# Developing Ratatoskr Extractor

> Status: Proposed  
> Last reviewed: 2026-08-20

The repository is in architecture bootstrap. The extractor, browser worker, parsers, fixtures, and benchmarks are not implemented.

## Intended toolchain

Rust/Tokio, Reqwest/Rustls, html5ever/scraper, PDF adapters, bounded CPU work, SQLx/PostgreSQL, BlobStore, Chromium behind a `BrowserRenderer`, tracing, testcontainers, fuzz/property tests, and benchmark tooling.

## Code size limits

There is no code here yet, so no limit is enforced yet. The commit that brings the first manifest brings the configuration that carries the limits with it: `clippy.toml` beside a `Cargo.toml`, `eslint.config.js` beside a `package.json`. `fleet.yml` fails the gate when a manifest arrives without one, so the rule has a check behind it and not only this paragraph.

`ratatoskr-workspace/docs/QUALITY_GATES.md` holds the numbers the repositories with code use today, the command that measured each one, and the limits that were rejected with the reason. Read it before you choose numbers, then measure this tree. Each limit is set at the worst case the tree already has, so that the check fails on a regression and not on work that has not been done yet.

## Workflow

1. Classify the source and preserve the original URL.
2. Change one stage without bypassing shared fetch, parse, quality, or provenance logic.
3. Add or update a licensed/synthetic golden fixture.
4. Test SSRF, redirects, limits, malformed input, determinism, and cancellation.
5. Compare quality, latency, requests, bytes, CPU, memory, and browser escalation against baseline.

The first scaffold PR must document exact build, format, lint, unit, integration, fuzz, corpus, benchmark, browser, and migration commands. LLM credentials must never be needed because LLM interpretation is outside this repository.

## What a clone needs before you plan a change

A change is planned with OpenSpec, which is a CLI a clone installs for itself. Use the version
`.github/workflows/openspec.yml` pins, so your terminal and the gate answer the same:

```bash
npm install --global @fission-ai/openspec@1.10.0
```

Cross-repository behaviour lives in a store, and registering one is per-machine state that no
repository can turn on for you — the same kind of step as `git config core.hooksPath .githooks`:

```bash
git clone git@github.com:po4yka/ratatoskr-workspace.git <path>
openspec store register <path> --id ratatoskr-workspace
```

`openspec doctor` reports whether both are in place.
