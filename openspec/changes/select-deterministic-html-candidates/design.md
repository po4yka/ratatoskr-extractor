## Context

`crates/document-ir` parses one HTML5 DOM and currently collects every heading and paragraph under
the fixed `html_primitives` strategy. The database already stores candidate metrics, scores, reasons,
and an optional artifact, but it has no selected marker and the worker does not write candidate rows.
See `proposal.md` for motivation and the two delta specs for behavior.

## Goals / Non-Goals

**Goals:**

- Preserve one fetch and one bounded DOM parse.
- Return the selected Document IR and all candidate decisions from one synchronous conversion.
- Keep scoring independent of time, randomness, host rules, and network state.
- Leave enough persisted evidence to explain a winner or rejection.

**Non-Goals:**

- Readability feature parity, JSON-LD, publisher selectors, PDF, or browser rendering.
- A broad production corpus, fuzz target, benchmark suite, or automatic threshold tuning.
- Any shared contract or event-shape change.

## Decisions

### Candidate extraction stays in `crates/document-ir`

Add internal candidate and evaluator modules beside the existing DOM. The public conversion returns
an `HtmlExtraction` containing `document: Document` and candidate decisions. A compatibility wrapper
is not kept because all callers are in this repository and can move in one commit. This avoids a new
crate and keeps parsing, block construction, and scoring under one resource boundary.

Alternative: create one trait and crate per strategy. Rejected because three fixed in-process
strategies have no independent deployment or dependency and the abstraction adds no current seam.

### Strategies inspect the same tree in a stable order

The strategies are `semantic`, `readability`, and `density`. Semantic uses the first non-empty
`article`, then `main`. Readability chooses the non-boilerplate container with the greatest body text
after link text is discounted. Density chooses the container with the greatest text per descendant
element. DOM order breaks container ties. Each strategy emits headings and paragraphs in source order.

Alternative: import a second extraction framework. Rejected because it would add a dependency and
usually construct another DOM, which breaks the central invariant.

### Scores use fixed-point integers

`QualityScore` is an integer from 0 through 1000. Components are capped integer ratios: text volume
(300), paragraph distribution (200), non-link share (200), non-boilerplate share (200), and title
agreement (100). Acceptance requires at least 120 normalized text characters and a score of 350.
The winner has the highest score; equal scores prefer semantic, then readability, then density.
Persistence converts the integer total to the existing unit-interval database value and stores every
integer component in `metrics`.

Alternative: floating-point weights. Rejected because fixed-point arithmetic makes exact equality,
fixtures, and tie behavior simpler.

### Selection is part of the candidate record

Add a non-null `selected boolean` with a partial unique index that allows one selected candidate per
run. The editable `schema.sql` changes in place; there is no migration. Successful completion inserts
all candidate decisions, marks one selected, stores the accepted Document IR artifact, updates the
run, and enqueues both completion events in the existing transaction. Quality rejection uses the
same terminal boundary but stores only the raw artifact, leaves every candidate unselected, and
enqueues only the failed operation report.

Alternative: infer selection only from Document provenance. Rejected because rejected runs have no
Document and operators still need the decision evidence.

### The corpus is small and synthetic

Four minimized fixtures cover the acceptance boundary: clean semantic article, noisy layout,
malformed document, and login shell. The corpus test reads a table of expected winner or rejection and
score range. It does not add a generic corpus framework.

## Risks / Trade-offs

- [Simple heuristics mis-rank unusual pages] → Keep rejection conservative and require corpus evidence
  for weight or threshold changes; add source-specific strategies only after measured failures.
- [Readability and density can produce duplicate blocks] → Keep them as separate evidence; stable
  scoring and tie order make the duplicate harmless.
- [Candidate persistence enlarges the terminal transaction] → Keep the three-row candidate set
  bounded and perform no network, parsing, or blob writes while the transaction is open.

## Migration Plan

Deploy the schema definition and code together while development databases are disposable. Existing
callers move to the new conversion result in the same repository commit. Roll back by reverting the
code and recreating the development database from the prior schema; no wire consumer changes.
