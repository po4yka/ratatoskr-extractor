#!/usr/bin/env bash
set -euo pipefail

targets=(html_parser pdf_extraction url_classification)
if [[ "$#" -eq 1 ]]; then
  case "$1" in
    html_parser|pdf_extraction|url_classification) targets=("$1") ;;
    *)
      printf '%s\n' 'usage: tools/run-fuzz-smoke.sh [html_parser|pdf_extraction|url_classification]' >&2
      exit 2
      ;;
  esac
elif [[ "$#" -ne 0 ]]; then
  printf '%s\n' 'usage: tools/run-fuzz-smoke.sh [html_parser|pdf_extraction|url_classification]' >&2
  exit 2
fi

temporary_directory=$(mktemp -d)
trap 'rm -rf "$temporary_directory"' EXIT
fuzz_toolchain="${FUZZ_TOOLCHAIN:-nightly}"
repository_root=$(pwd)
temporary_repository="$temporary_directory/repository"
mkdir -p "$temporary_repository"
cp Cargo.toml Cargo.lock "$temporary_repository"
cp -R fuzz "$temporary_repository/fuzz"
cp -R "$repository_root/crates" "$temporary_repository/crates"

for target in "${targets[@]}"; do
  log="$temporary_directory/$target.log"
  (
    cd "$temporary_repository/fuzz"
    cargo "+$fuzz_toolchain" fuzz run "$target" -- -max_total_time=15
  ) >"$log" 2>&1 || {
    tail -n 200 "$log" >&2
    exit 1
  }
  printf '%s\n' "fuzz target completed without findings: $target"
done
