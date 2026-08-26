## 1. Hosted gate repair

- [x] 1.1 Run a failing shell check against the pre-change performance wrapper without `build-gate`, and observe its expected exit 127.
- [x] 1.2 Make the wrapper use `build-gate` only when available and install cargo-fuzz with the repository toolchain while retaining the pinned nightly fuzz execution; run the wrapper and bounded fuzz smoke.

## 2. Delivery

- [x] 2.1 Run the hosted-required local checks and validate this tooling-only change before archiving it.
