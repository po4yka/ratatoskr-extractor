## Why

Four independent defects are failing this repository's CI right now, all on commit `4ade491` ("feat: assign stable document block identifiers") or the tree it left behind. Three were named by the audit that opened this change; the fourth was found while running the full documented gate to verify the other three, since CI's own fail-fast `cargo test --workspace` never reached it.

`fuzz/` is its own standalone Cargo workspace with its own direct git-rev pins on `ratatoskr-identifiers` and `ratatoskr-document-contracts`, hardcoded to `rev = "d56c6891ce9b4fca2c65d43205080d9a666ab5a0"` since `fuzz/` was added. `4ade491` bumped the root workspace's pin for the same crates to `rev = "4929b9659dfb80c768ae6340ef7fd981132dfaf3"` but did not touch `fuzz/Cargo.toml`. Cargo treats the two revs as distinct source IDs, so the fuzz targets construct structurally-identical `DocumentId`, `DocumentAddress`, and `BlobRef` values that fail to unify, producing `error[E0308]: mismatched types` in `fuzz_targets/html_parser.rs` and `fuzz_targets/pdf_extraction.rs`. This is stale-pin drift, not an API-shape break: the CI `fuzz` job (run `33052828461`, job `98452243250`) fails accordingly.

`4ade491` added a new `block_id: BlockId` field to `DocumentBlock::Heading` and `DocumentBlock::Paragraph`, deterministically derived from `SHA-256(domain-separation-string || content_digest || block-ordinal)` in the single `assemble_document` constructor every extraction path (HTML, PDF, provider, YouTube) routes through. The commit updated every unit test to match but did not regenerate `tools/corpus/expected/*.json`, so `cargo test -p ratatoskr-extractor-corpus --test golden` fails: the five committed golden fixtures have no `block_id` key while every current extraction output does. The CI `gate` job (run `33052828461`, job `98452243561`) fails accordingly.

`Cargo.lock` locks `chacha20 0.10.1`, reached transitively through `rand 0.10.2` from the pinned `async-nats = "=0.50.0"` (browser-worker, eventing, extractor service) and through `lopdf`/`pdf-extract` (the PDF adapter). crates.io yanked `chacha20 0.10.1` (and `0.10.0`) on 2026-08-27T17:51:13Z UTC, 15 minutes after this repository's last green `advisories` run (`33099308271`, 2026-08-27T17:36:58Z). `deny.toml` sets `[advisories] yanked = "deny"`, so `cargo deny check` now fails on the locked resolution wherever it runs. A manual `workflow_dispatch` of `advisories.yml` against the still-unfixed head (run `33190643665`, job `98914931562`) reproduces this in CI, since no push had triggered the workflow since the yank landed.

`4ade491` also added one line to `crates/eventing/src/lib.rs` (`ai_archive_import_summary: None,` inside a struct literal), pushing it from 850 to 851 lines. `ci.yml`'s `gate` job enforces an 850-line ratchet over every tracked `.rs` file, documented in `DEVELOPMENT.md` as "set at the worst case the tree already has, so that the check fails on a regression." `cargo test --workspace --locked` (the step immediately before the ratchet in `ci.yml`) fails fast on the golden-corpus regression above and stops scheduling further test binaries (no `--no-fail-fast`), so the CI run that failed on `4ade491` never reached the ratchet step and never reported this second regression. Running the full documented gate locally to verify the three defects above surfaced it.

## What Changes

- Bump `fuzz/Cargo.toml`'s two `rev =` pins for `ratatoskr-document-contracts` and `ratatoskr-identifiers` to `4929b9659dfb80c768ae6340ef7fd981132dfaf3`, matching the root workspace, and regenerate `fuzz/Cargo.lock`.
- Add a `ci.yml` `fuzz`-job step that asserts `fuzz/Cargo.toml`'s contracts revision matches the root workspace's, and a fast `cargo check --locked` step for the `fuzz` crate on the pinned stable toolchain, both running before the nightly toolchain install — so this class of drift fails in seconds instead of after the fuzz job's nightly/cargo-fuzz build.
- Re-bless all five committed golden corpus fixtures (`html-semantic`, `pdf-direct`, `hacker-news`, `reddit`, `youtube-transcript`) via `cargo run --locked -p ratatoskr-extractor-corpus --bin corpus-bless -- <case>`, adding only the new `block_id` key per heading/paragraph block. `content_digest`, `title`, `text`, and `provenance` are unchanged in every case.
- Run `cargo update -p chacha20 --precise 0.10.2` to move the locked resolution off the yanked version, with no `Cargo.toml` edit since `chacha20` is transitive and `async-nats = "=0.50.0"` stays pinned exactly as before.
- Deduplicate the three identical 8-line closures in `crates/eventing/src/lib.rs` that map a `(Uuid, Uuid, Uuid, String)` query row into a `CompletionContext` into one `impl From<CompletionRow> for CompletionContext`, then run `cargo fmt --all` to canonicalize. This is a pure, behaviour-preserving deduplication (same field-for-field construction, now written once) that brings the file from 851 to 843 lines — chosen over deleting an unrelated line elsewhere, because it fixes the actual cause (three copies of the same 8 lines) rather than making room by coincidence.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. `block_id` is an existing, already-shipped Document IR field (added in `4ade491`); this change only makes the committed golden fixtures agree with the code that has been live since that commit. The `eventing` deduplication is a pure refactor with identical behaviour, covered by the crate's own existing tests. No contract, schema, or externally-visible behaviour changes here. `skip_specs: true` is set in the change manifest.

## Impact

- `fuzz/Cargo.toml`, `fuzz/Cargo.lock` (contracts revision pin).
- `.github/workflows/ci.yml` (`fuzz` job: rev-drift guard and fast stable-toolchain check).
- `DEVELOPMENT.md` (documents the new `fuzz`-job steps).
- `tools/corpus/expected/*.json` (five files, additive `block_id` keys only).
- `Cargo.lock` (`chacha20` entry only: version and checksum).
- `crates/eventing/src/lib.rs` (deduplicates three identical closures into one `From` impl; no behaviour change).
- No other source (`.rs`) behaviour changes; no other lockfile entry moves.
