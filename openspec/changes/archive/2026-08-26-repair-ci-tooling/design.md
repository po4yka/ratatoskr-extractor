## Context

The hosted run on commit 5129569 failed before performance measurement because `build-gate` is unavailable on Ubuntu, and before fuzzing because cargo-fuzz is compiled with a pinned nightly incompatible with its dependencies.

## Goals / Non-Goals

**Goals:** retain the full performance and fuzz gates on Linux and macOS.

**Non-Goals:** changing any quality threshold, skipping a gate, or changing product behavior.

## Decisions

Use `build-gate` only when it is installed; the command remains mandatory on the protected local Mac. Install cargo-fuzz using the repository toolchain and run fuzz targets with the existing pinned nightly.

## Risks / Trade-offs

- [Plugin install can drift from the fuzz compiler] → retain the pinned plugin version and nightly execution command.
