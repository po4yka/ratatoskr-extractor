## Why

The primitive HTML path keeps every heading and paragraph, so navigation and footer text can win by
volume. Extractor item 5 needs several candidates from the existing single DOM and one explainable,
deterministic decision before Knowledge starts to consume documents.

## What Changes

- Run semantic, readability-compatible, and text-density candidate extraction against one parsed DOM.
- Calculate bounded integer quality components, a total score, an acceptance decision, and a stable
  tie-break without network, time, or random inputs.
- Persist every candidate and the selected marker through the owned candidate records.
- Add a small synthetic calibration corpus for article, boilerplate-heavy, malformed, and low-quality
  HTML, and mark Extractor implementation-plan item 5 complete.
- Keep PDF extraction, browser escalation, broad fuzzing, and performance reporting outside this
  change.

## Capabilities

### New Capabilities

- `html-candidate-selection`: Candidate generation, quality components, deterministic selection,
  rejection, persistence, and calibration expectations.

### Modified Capabilities

- `document-ir`: Replace the primitive single-strategy restriction with the parse-once rule for
  several candidates and one selected Document IR.

## Impact

The change affects `crates/document-ir`, its corpus fixtures, the extraction worker, candidate
persistence calls, the editable `extractor.candidates` schema, and implementation documentation. It
changes no shared Document IR field, event payload, dependency, fetch count, or DOM parse count.
