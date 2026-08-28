## 1. Reproduce each failing gate

- [x] 1.1 Fuzz revision drift: reproduced locally with `cd fuzz && cargo check` on pre-change `fuzz/Cargo.toml` (this is CI configuration and a stale dependency pin, not a new unit test, so the failing gate command is the failing test, per `openspec/config.yaml`'s "a task that cannot start from a failing test states why" rule) — three `error[E0308]: mismatched types` for `DocumentAddress`, `BlobRef`, and (transitively) `DocumentId`, matching CI run `33052828461` job `98452243250` (`fuzz`) verbatim.
- [x] 1.2 Golden corpus: reproduced locally with `build-gate cargo test -p ratatoskr-extractor-corpus --test golden --locked` — `golden_corpus_verifies_committed_outputs` panics with "every committed corpus case must verify", matching CI run `33052828461` job `98452243561` (`gate`).
- [x] 1.3 Yanked `chacha20`: reproduced locally with `build-gate cargo deny --locked check advisories` — `error[yanked]: detected yanked crate` at `Cargo.lock:37`, resolving through `rand v0.10.2 <- async-nats v0.50.0` and `rand v0.10.2 <- lopdf v0.42.0 <- pdf-extract v0.12.0`. Also reproduced in CI itself: triggered `advisories.yml` by hand against the still-unfixed head (`workflow_dispatch`, run `33190643665`, job `98914931562`, conclusion `failure`), since no push had re-run the schedule since the last green run (`33099308271`) predates the yank.
- [x] 1.4 Eventing file-size ratchet: this is configuration enforcement over already-committed code, not a new unit test (same "states why" exemption as 1.1). Discovered while running the full `DEVELOPMENT.md` gate to verify 1.1-1.3: `git ls-files -z "*.rs" | xargs -0 -r wc -l | awk '$2 != "total" && $1 > 850 { print; bad = 1 } END { exit bad }'` printed `851 crates/eventing/src/lib.rs` and exited 1. Never seen in CI because `cargo test --workspace --locked` in `ci.yml`'s `gate` job fails fast on 1.2 and stops scheduling later steps (no `--no-fail-fast`), so the ratchet step in the same run never executed.

## 2. Fix the fuzz workspace's stale revision pin

- [x] 2.1 Bumped both `rev =` values in `fuzz/Cargo.toml` to `4929b9659dfb80c768ae6340ef7fd981132dfaf3`, matching the root workspace's `[workspace.dependencies]` pin.
- [x] 2.2 Regenerated `fuzz/Cargo.lock` (`rm fuzz/Cargo.lock && cargo generate-lockfile` inside `fuzz/`, since the old lockfile's entries were keyed to the abandoned rev and made `cargo update -p` report an ambiguous specification). `cargo check --locked` inside `fuzz/` now succeeds with no errors.
- [x] 2.3 Added a `fuzz/Cargo.toml`-vs-root-`Cargo.toml` revision-drift guard and a fast `cargo check --locked` step to the `fuzz` job in `ci.yml`, both ahead of the nightly toolchain install, and documented both in `DEVELOPMENT.md`.

## 3. Re-bless the golden corpus

- [x] 3.1 Ran `cargo run --locked -p ratatoskr-extractor-corpus --bin corpus-bless -- <case>` for `html-semantic`, `pdf-direct`, `hacker-news`, `reddit`, and `youtube-transcript`.
- [x] 3.2 Reviewed the diff: `git diff --stat -- tools/corpus/expected/` reports 15 insertions, 0 deletions across the five files; every changed line adds one `"block_id": "<uuid>"` key immediately after a `"kind": "heading"` or `"kind": "paragraph"` line, with `content_digest`, `title`, `text`, and `provenance` byte-identical to before.
- [x] 3.3 `build-gate cargo test -p ratatoskr-extractor-corpus --test golden --locked` now passes.

## 4. Move the yanked `chacha20` off the locked resolution

- [x] 4.1 `cargo update -p chacha20 --precise 0.10.2` — a 4-line `Cargo.lock` diff (`chacha20`'s `version` and `checksum` fields only). No `Cargo.toml` edit.
- [x] 4.2 `build-gate cargo deny --locked check advisories` now reports `advisories ok`; `build-gate cargo deny --locked check` reports `advisories ok, bans ok, licenses ok, sources ok`.

## 5. Fix the eventing file-size ratchet

- [x] 5.1 Replaced the three identical 8-line `.map(|(command_id, operation_id, owner_id, correlation_id)| CompletionContext { ... })` closures in `crates/eventing/src/lib.rs` (in `fail_run`, `complete_document`, `reject_quality`) with `.map(CompletionContext::from)`, backed by a new `type CompletionRow = (uuid::Uuid, uuid::Uuid, uuid::Uuid, String);` and `impl From<CompletionRow> for CompletionContext`.
- [x] 5.2 Ran `cargo fmt --all` to canonicalize the new `impl From` block's formatting, then confirmed `cargo fmt --all -- --check` passes with no diff. File is now 843 lines.
- [x] 5.3 `git ls-files -z "*.rs" | xargs -0 -r wc -l | awk '$2 != "total" && $1 > 850 { print; bad = 1 } END { exit bad }'` now exits 0.
- [x] 5.4 `crates/eventing/tests/completion.rs` (the test that exercises `complete_document`, hence the refactored closure) passed in the full workspace test run in task 6.1.

## 6. Verify the full gate

- [x] 6.1 Ran the full documented gate from `DEVELOPMENT.md` end to end against PostgreSQL 17 on `127.0.0.1:5434`, JetStream NATS on `127.0.0.1:24222` (`EXTRACTOR_TEST_NATS_URL` override — the documented default port `4222` was already bound by an unrelated repository's container on this shared host), and system Google Chrome via `CHROME_BIN`: `cargo fetch --locked`, `cargo deny --locked check`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo build --workspace --locked`, `cargo test --workspace --locked`, `cargo test -p ratatoskr-extractor-corpus --locked`, `cargo run --locked -p ratatoskr-extractor-corpus --bin shadow-report -- --check`, `tools/run-corpus-performance.sh --check`, `cargo test --workspace --locked --doc`, `cargo build --workspace --locked --release`, and the 850-line file-size ratchet. All steps passed; 114 test binaries reported `test result: ok`, 177 tests passed, 0 failed, 0 ignored, across the workspace.
- [x] 6.2 This run followed an earlier, abandoned attempt at the same gate that hit sustained host-wide disk exhaustion (`ld`/`rustc` `No space left on device` on a shared multi-agent box, `df -h /` repeatedly between 0 and ~1.3Gi free out of 926Gi) — an infrastructure condition unrelated to this change, confirmed by a non-destructive `docker builder prune -f` / `docker image prune -f` (freed only unused, inactive cache) and dozens of retries at reduced `--jobs` before the host recovered on its own to 130+Gi free, after which the full gate above ran clean in one pass.
- [x] 6.3 `uvx zizmor@1.29.0 --persona pedantic --min-severity low .github/workflows/` reports no findings.
- [x] 6.4 `openspec validate --all --strict` and `openspec validate --archived` both pass.

## 7. Latent weaknesses considered and left alone

- [x] 7.1 `--no-fail-fast` for `cargo test --workspace --locked` (audit: `gate-does-not-gate`, severity low): out of scope as a standalone item, though its consequence (task 1.4's ratchet failure going unreported) is exactly what this change's task 5 fixes. Adding the flag itself is left for a separate change since it is not the same defect as any of the four fixed here and is rated low severity.
- [x] 7.2 The one-off `cargo install cargo-fuzz` rustix build failure (audit: `flaky-test`, severity low): out of scope — zero recurrence in the last two fuzz-job runs, and independent of source code.
- [x] 7.3 The single `zizmor` `startup_failure` run with zero steps executed (audit: `unreachable-job`, severity low): out of scope — one-off runner-provisioning failure, never repeated in 29 other sampled runs, not driven by anything in this repository's `zizmor.yml`.
