## Context

See [proposal.md](proposal.md). The current repository has four small HTML calibration fixtures and
crate-local PDF/provider/transcript tests, but no single offline corpus or resource evidence. The
deployment unit gives extractor `MemoryHigh=768M`, `MemoryMax=1G`, and two CPUs; the frozen
deployment-target document is a budget, not a machine to operate.

## Goals / Non-Goals

**Goals:**

- Keep corpus inputs, expected IR, fuzz seeds, and performance baseline in version control.
- Exercise real public conversion entry points with no network, service, Chrome, or LLM dependency.
- Make accidental IR changes fail a normal test; make intentional changes explicit and reviewable.
- Bound fuzz CI cost while retaining inputs suitable for longer local campaigns.
- Evaluate corpus resource use against portable, documented limits and the deployment memory budget.

**Non-Goals:**

- Live-capture refresh, legacy shadow comparison, OCR, browser rendering benchmarks, or semantic
  quality judgments.
- Changing Document IR, extraction thresholds, parser behavior, the database, or shared contracts.

## Decisions

### A dedicated tooling crate owns corpus execution

Add a small workspace member under `tools/corpus`. It depends on the existing HTML, PDF, provider,
YouTube, URL-routing, contracts, and identifier crates and exposes the same case runner to a
read-only integration test, a bless binary, and a performance binary. Source documents live beside
the tooling crate as minimized synthetic fixtures; `expected/` contains canonical JSON Document IR.

This keeps test-only orchestration out of production crates and makes every corpus case exercise
the public extraction boundary. Extending each individual crate's tests was rejected because it
would duplicate fixture ownership and could not produce one cross-path performance report.

The initial cases cover the two currently defined block kinds (`heading` and `paragraph`), static
and malformed/multilingual HTML, direct PDF, Hacker News, Reddit, and YouTube transcript input.
Their manifests carry an input kind, fixed document identity/source address, and source media type,
so adding a contract block kind or adapter requires adding a case rather than relying on convention.

### Golden verification is immutable; blessing is a narrow CLI action

`cargo test -p extractor-corpus --test golden` reads source and expected output and never writes.
`cargo run -p extractor-corpus --bin corpus-bless -- <case>` is the only writing entry point; it
requires an exact case name, regenerates only its expected JSON, and prints the changed path. Review
of the resulting Git diff is intentionally outside automation. A blanket bless was rejected because
it could silently ratify unrelated output changes.

### Fuzz as an isolated cargo-fuzz package

Keep cargo-fuzz's nightly/libFuzzer integration in `fuzz/`, outside the normal workspace graph.
Each target constructs bounded valid envelope values around fuzzer-controlled hostile payloads,
then calls the real HTML parser, PDF extractor, or normalize/classify path. Committed seeds include
valid minimal documents plus malformed edge cases. CI installs the pinned nightly and cargo-fuzz,
runs all three targets for 15 seconds each with `-max_total_time`, and fails on any finding; local
campaigns may increase the duration without changing CI policy.

Using direct raw bytes alone was rejected because target setup and policy boundaries would be
under-exercised; full end-to-end service fuzzing was rejected because it would add DNS/network,
storage, and timing nondeterminism without improving parser coverage.

### Performance uses an offline repeatable workload and explicit ceilings

The performance binary runs every successful corpus case for a fixed warmup and sample count. It
records per-path count, throughput, p50/p95 latency, and sampled resident memory in JSON. A platform
wrapper supplies maximum RSS through the native `time` tool, avoiding unsafe process-stat code in a
workspace that forbids unsafe Rust. The committed baseline stores absolute ceilings: latency and
throughput limits derived from the recorded report with headroom, and `786432 KiB` maximum sampled
RSS, matching the service's 768 MiB `MemoryHigh` budget. `--check` rejects a threshold breach and
names the metric, observed value, and limit.

The check intentionally uses absolute ceilings, rather than percent deltas across developer macOS
and Linux CI runners. Browser, network, database, and compile time are excluded so the metric stays
stable and describes only conversion work.

## Risks / Trade-offs

- [Synthetic fixtures miss real-site variation] → Cases encode known route and block boundaries;
  item 10 remains responsible for legacy shadow evidence.
- [CI fuzz has shallow coverage] → Seeds are committed, targets run on every CI invocation, and
  documented local commands support longer campaigns.
- [RSS collection differs by host] → The wrapper normalizes native `time` output to KiB and the
  checked memory ceiling is tied to the deployment `MemoryHigh`, not a Mac-only measurement.
- [Golden outputs can be blessed incorrectly] → Bless requires an exact case and leaves a normal
  reviewable diff; tests never write expectations.

## Migration Plan

1. Add the tooling crate, corpus cases, expected outputs, fuzz package/seeds, baseline, and CI/docs.
2. Run verification, bounded fuzz, report check, and the existing full gate.
3. Merge as one repository-local quality capability. Rollback is a normal revert; it changes no
   persisted data, public contract, or deployment state.
