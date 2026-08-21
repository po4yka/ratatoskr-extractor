## ADDED Requirements

### Requirement: One parsed DOM feeds every HTML candidate

Extractor SHALL parse verified HTML bytes once and SHALL build every extraction candidate from that
same bounded DOM. Only the selected candidate SHALL become shared Document IR, and its provenance
SHALL name the selected strategy.

#### Scenario: candidate selection preserves parse-once behavior

- **WHEN** semantic, readability-compatible, and text-density candidates are evaluated
- **THEN** one parser invocation produces the DOM and the selected Document IR names the winning
  strategy in every block provenance entry

## REMOVED Requirements

### Requirement: Item 4 does not implement candidate scoring

**Reason**: Extractor implementation-plan item 5 now adds candidate scoring to the ordinary HTML path.

**Migration**: Callers keep using the same HTML conversion entry point and shared Document IR shape;
the fixed `html_primitives` provenance value is replaced by the selected strategy.
