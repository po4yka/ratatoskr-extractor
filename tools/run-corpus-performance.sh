#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != "--check" || "$#" -ne 1 ]]; then
  printf '%s\n' 'usage: tools/run-corpus-performance.sh --check' >&2
  exit 2
fi

temporary_directory=$(mktemp -d)
trap 'rm -rf "$temporary_directory"' EXIT
report="$temporary_directory/report.json"
timing="$temporary_directory/time.txt"

case "$(uname -s)" in
  Darwin)
    /usr/bin/time -l build-gate -- cargo run --locked -p ratatoskr-extractor-corpus \
      --bin corpus-performance -- --iterations 100 --max-rss-kib 0 >"$report" 2>"$timing"
    max_rss_kib=$(awk '/maximum resident set size/ { print int($1 / 1024) }' "$timing")
    ;;
  Linux)
    /usr/bin/time -f '%M' -o "$timing" build-gate -- cargo run --locked -p ratatoskr-extractor-corpus \
      --bin corpus-performance -- --iterations 100 --max-rss-kib 0 >"$report"
    max_rss_kib=$(tr -d '[:space:]' <"$timing")
    ;;
  *)
    printf '%s\n' "unsupported host for RSS measurement: $(uname -s)" >&2
    exit 2
    ;;
esac

if [[ ! "$max_rss_kib" =~ ^[0-9]+$ ]]; then
  printf '%s\n' 'could not collect maximum resident memory' >&2
  exit 1
fi

build-gate -- cargo run --locked -p ratatoskr-extractor-corpus --bin corpus-performance -- \
  --iterations 100 --max-rss-kib "$max_rss_kib" --check
