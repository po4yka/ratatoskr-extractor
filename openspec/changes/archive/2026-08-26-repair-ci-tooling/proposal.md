## Why

The hosted gate cannot complete because its Linux runner lacks the macOS-only build gate command and installs the pinned fuzz plugin with an incompatible nightly compiler.

## What Changes

- Run performance measurements with the local build gate only where that command exists.
- Install the pinned cargo-fuzz plugin with the repository toolchain while preserving the pinned nightly for fuzz execution.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None.

## Impact

This is CI/tooling-only; it changes no extraction behavior, contracts, routing, or production traffic.
