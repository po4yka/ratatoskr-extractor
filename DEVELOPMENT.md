# Developing Ratatoskr Extractor

> Status: Active
> Last reviewed: 2026-08-21

The Rust workspace implements plan items 1 through 11: foundation, URL/SSRF policy, bounded fetch,
parse-once HTML candidates, deterministic quality selection, Document IR, PostgreSQL/JetStream
inbox/outbox integration, direct PDF extraction, the Hacker News and Reddit provider adapters with
link-post resolution, and the isolated browser worker whose escalation runs through one gated
policy (`render.enabled`, `render.allowed_hosts`, `render.max_escalations_per_day`).
PDFs without a text layer degrade explicitly; OCR stays out of scope. Delegated platform routes
(GitHub, X, Instagram, Threads, Telegram) remain unimplemented here.

## Toolchain and gate

`rust-toolchain.toml` pins Rust 1.97. Every command uses the committed lock file.

### Rust - the CI gate

```bash
cargo fetch --locked
cargo deny --locked check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked
cargo test --workspace --locked
cargo test -p ratatoskr-extractor-corpus --locked
cargo test --workspace --locked --doc
cargo build --workspace --locked --release
```

The corpus report is an additional gate command because it collects maximum resident memory through
the native platform `time` utility:

```bash
tools/run-corpus-performance.sh --check
```

### Golden corpus and fuzzing

`tools/corpus` owns five offline repository-owned fixtures that exercise HTML, direct PDF, Hacker
News, Reddit, and YouTube transcript conversion. Read-only verification runs with the corpus test.
An expected Document IR changes only through one explicitly named case, then its diff must be
reviewed:

```bash
cargo run --locked -p ratatoskr-extractor-corpus --bin corpus-bless -- html-semantic
```

The CI `fuzz` job uses `nightly-2026-06-11` and `cargo-fuzz 0.13.1` to run the committed seed corpus
for HTML, PDF, and URL classification targets for 15 seconds each. For a local smoke run:

```bash
tools/run-fuzz-smoke.sh
```

### Test environment

Integration tests require the services they exercise: PostgreSQL on `5434` and JetStream NATS on
`4222` (both from `compose.yaml`), and a Chrome or Chromium binary for the browser-worker tests —
set `CHROME_BIN`, or keep a Chromium on `PATH`. CI provisions all three; a gate run without them
fails rather than skips. Non-default service locations are honoured through
`EXTRACTOR_TEST_DATABASE_URL` and `EXTRACTOR_TEST_NATS_URL`.

The browser worker reads flat `BROWSER_*` environment variables; the deployment examples carry the
full set. `BROWSER_CHROME_BIN` selects the Chromium executable and
`BROWSER_MAX_JOBS_PER_PROCESS` (default 500) ends the process cleanly after that many terminal
render jobs so its supervisor restarts it with fresh Chromium.
The `browser` compose profile enforces a 2-CPU, 1-GiB, and 256-PID ceiling with `restart: always`;
the job limit bounds slow leaks before the cgroup limits have to stop the process.

The file-size ratchet is the one check that Cargo cannot express:

```bash
git ls-files -z "*.rs" | xargs -0 -r wc -l | awk '$2 != "total" && $1 > 850 { print; bad = 1 } END { exit bad }'
```

## Code size limits

`clippy.toml` carries the function, block, signature, and type limits copied from Platform. CI also
rejects a tracked Rust source file above 850 lines.

`ratatoskr-workspace/docs/QUALITY_GATES.md` holds the numbers the repositories with code use today, the command that measured each one, and the limits that were rejected with the reason. Read it before you choose numbers, then measure this tree. Each limit is set at the worst case the tree already has, so that the check fails on a regression and not on work that has not been done yet.

## Workflow

1. Classify the source and preserve the original URL.
2. Change one stage without bypassing shared fetch, parse, quality, or provenance logic.
3. Add or update a licensed/synthetic golden fixture.
4. Test SSRF, redirects, limits, malformed input, determinism, and cancellation.
5. Run the offline corpus report; its p95/throughput thresholds and 768 MiB RSS ceiling are in
   `tools/corpus/performance/baseline.json`.

LLM credentials are not needed because LLM interpretation is outside this repository.

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

## The Rust skills in this repository

`.agents/skills/` holds eighteen Rust skills vendored from `po4yka/rust-skills`, and
`.claude/skills/` symlinks to them. Unlike the steps above this needs nothing from your machine: the
files are in the tree, so a fresh clone already has them.

Update them with the catalogue and never by hand:

```bash
npx skills update
```

That rewrites `.agents/skills/` and `skills-lock.json` from the catalogue. Run it in one repository,
read the diff, then apply the same change to every Ratatoskr repository whose stack is Rust.
`ratatoskr-workspace/.github/workflows/drift.yml` fails when one copy differs from the others.
