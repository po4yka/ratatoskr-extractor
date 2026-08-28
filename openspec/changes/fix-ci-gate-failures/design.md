## Context

See [proposal.md](proposal.md). Four unrelated root causes are bundled into one change because they were discovered together while restoring this repository's CI and are each a small, independent, tooling/fixture/dead-weight repair with no user-visible behavioural change of its own. Three came from the originating audit; the fourth (the eventing file-size ratchet) surfaced only once the full documented gate was run locally, since CI's own fail-fast `cargo test --workspace` had never reached that step on the commit that introduced it.

## Goals / Non-Goals

**Goals:**

- Restore a green `ci` (`gate` and `fuzz` jobs) and a green `cargo deny check advisories` with the smallest diff that fixes each root cause.
- Make the fuzz-crate revision-drift class of failure fail fast and by name the next time it happens, instead of silently compiling against two different dependency graphs.

**Non-Goals:**

- Do not change `DocumentBlock`, `assign_block_ids`, or any other already-shipped Document IR behaviour from `4ade491`. That commit's design is correct; this change only catches up the fixtures and pins that were left behind.
- Do not widen or change any direct dependency version requirement in `Cargo.toml`; `chacha20` is transitive.
- Do not touch `fleet.yml`: it is byte-identical across all seventeen repositories and drift-checked by `ratatoskr-workspace/.github/workflows/drift.yml`. The fuzz-workspace revision-drift guard is repository-specific (`fuzz/` does not exist as a standalone workspace anywhere else in the fleet) and belongs in this repository's own `ci.yml`.
- Do not add `--no-fail-fast` to `cargo test --workspace --locked`, retire the flaky `cargo install cargo-fuzz` step, or investigate the one `zizmor` `startup_failure` run. All three are pre-existing latent weaknesses the audit rated `low` severity and non-recurring or independent of this change; fixing them is out of scope here.

## Decisions

**Fuzz revision pin.** `fuzz/Cargo.toml`'s two `rev =` values move to the root workspace's current pin, and `fuzz/Cargo.lock` is regenerated from scratch (`rm fuzz/Cargo.lock && cargo generate-lockfile` inside `fuzz/`) rather than a targeted `cargo update -p`, because the old lockfile's entries are keyed to the now-abandoned rev and Cargo reports the package specification as ambiguous when both revs are present. `fuzz/Cargo.lock` is not read by any other gate (`cargo deny --locked check` in `ci.yml` and `advisories.yml` both run from the repository root against the root `Cargo.lock`, and `fuzz/` is a separate workspace `cargo deny` never descends into), so the regenerated lockfile's other transitive bumps carry no gate risk; one of them incidentally also moves `chacha20` to `0.10.2` inside the fuzz workspace, consistent with the root-workspace fix below.

**Fuzz coverage gap.** The audit's own root-cause finding is that a stale rev pin compiles cleanly until the fuzz job's ~15-minute nightly/cargo-fuzz build reaches it. Two cheap, repository-local `ci.yml` steps close this: a grep-based assertion that `fuzz/Cargo.toml`'s `ratatoskr-contracts` rev(s) equal the root workspace's (catches the drift the moment the root pin next changes, regardless of source-level compatibility), and a `cargo check --locked` against `fuzz/` on the already-pinned stable toolchain (catches any resulting type error in seconds). Both run before "Install pinned fuzz toolchain" in the `fuzz` job. `DEVELOPMENT.md` documents both, since the "gate and DEVELOPMENT.md are one list" check in `ci.yml` only compares the `gate` job's `cargo` commands against `DEVELOPMENT.md`'s fenced gate block, not the `fuzz` job — no coupling to break, but keeping the docs in sync is worth the four sentences.

**Golden corpus re-blessing.** Re-run the documented `corpus-bless` procedure for all five cases and inspect the diff before committing, per `DEVELOPMENT.md`'s own instruction ("its diff must be reviewed"). The resulting diff is confirmed to add exactly one `"block_id": "<uuid>"` key per heading/paragraph block per case (15 insertions across 5 files, zero deletions, zero other field changes) — `git diff` was inspected line by line as part of this change, not assumed from the audit's prediction.

**Yanked `chacha20`.** `cargo update -p chacha20 --precise 0.10.2`, which SemVer-bumps only the `chacha20` lockfile entry (`rand 0.10.2` already accepts `chacha20 ^0.10`, so no dependent's requirement needs to change). An alternative — pinning `async-nats` to a different version to reshape the transitive graph — was considered and rejected: the problem is entirely in a transitive leaf, and there is no evidence any other `async-nats` version in the allowed range resolves a materially different graph.

**Eventing file-size ratchet.** `crates/eventing/src/lib.rs` had three byte-identical 8-line closures — one per call site in `fail_run`, `complete_document`, and `reject_quality` — each mapping the same `(Uuid, Uuid, Uuid, String)` query row into a `CompletionContext` by hand. `4ade491`'s one-line addition elsewhere in the file was the proximate trigger, but the actual cause of the file being oversized is this pre-existing triplication. The fix is `impl From<CompletionRow> for CompletionContext` (a `CompletionRow` type alias replaces the repeated tuple-type spelling) plus `.map(CompletionContext::from)` at each of the three call sites, then `cargo fmt --all` to canonicalize the result — 851 lines down to 843, with `cargo fmt --all -- --check` passing on the canonicalized output. An alternative considered and rejected: deleting a blank line elsewhere in the file to shave exactly one line. That would satisfy the ratchet by coincidence without addressing why the file is large, and `rustfmt`'s own heuristics (`fn_call_width` under `max_width = 100`) rule out silently collapsing any of the file's existing multi-line calls onto one line — `cargo fmt --all -- --check` already passed on the pre-existing formatting, so any call already sits at rustfmt's canonical width for its argument list.

## Risks / Trade-offs

- [`fuzz/Cargo.lock`'s incidental transitive bumps (beyond `chacha20`) diverge further from the root `Cargo.lock` over time] → they are never compared; `fuzz/` has always been an independently-resolved workspace, and this change does not add or remove that independence.
- [The new `ci.yml` rev-drift check is a grep, not a semantic comparison] → it is intentionally narrow: it reads five `rev = "<40-hex>"` occurrences from two files and compares the resulting sets, which is exactly the invariant that broke. A more general "these two Cargo.tomls must resolve to the same dependency graph" check would need `cargo metadata` against two different workspace roots and is disproportionate to the failure mode being guarded against.
- [`chacha20 0.10.2` itself gets yanked later] → the same `cargo deny check` gate catches it again immediately; no different handling needed.

## Migration Plan

Commit all four groups of changes together (fuzz pin + lock, ci.yml + DEVELOPMENT.md, corpus fixtures, root Cargo.lock) as one commit. No rollout ordering, no data migration, no rollback beyond reverting the commit.
