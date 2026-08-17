# Extractor domain model

## Terms

- **Extraction job:** one bounded attempt to obtain a source representation.
- **Source route:** provider adapter, PDF, generic HTML, or browser-required policy.
- **Fetch artifact:** immutable response/body metadata and content hash.
- **Parsed document:** one in-memory DOM or document representation.
- **Candidate:** structured content proposed by an extraction algorithm.
- **Quality decision:** score, reasons, threshold, and accepted/rejected state.
- **Document IR:** canonical block structure independent of Markdown/HTML rendering.
- **Provenance span:** relationship from IR content to source artifact/selector/range.

## Lifecycle

`requested -> classified -> fetched -> parsed -> evaluated -> accepted | escalated -> rendered -> evaluated -> completed | failed`

## Invariants

1. Ordinary HTML is fetched once and parsed once per source version.
2. LLMs never determine extracted facts.
3. Every accepted block has traceable provenance or an explicit limitation.
4. Browser output is untrusted input, not authority.
5. Partial/failed results never overwrite a previously verified artifact silently.
6. Quality decisions are deterministic for fixed inputs and versions.
