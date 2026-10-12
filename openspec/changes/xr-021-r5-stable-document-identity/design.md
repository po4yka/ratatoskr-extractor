## Context

The cross-repository decision lives in XR-021 CONTRACTS.md R5-02. This design records only the extractor-internal choices.

## Decisions

### D1. The identity is derived where the digest is computed

`assemble_document` is the one constructor every extraction path already uses, and it is the only place that knows the content digest. It computes the digest, then calls `stable_document_id(owner, &digest)`. A parser input therefore carries the owner (a `UserId`) and cannot carry an identity, so no path can mint one. The construction mirrors `assign_block_ids` (first 16 bytes of a SHA-256, version nibble 8, RFC 4122 variant) with its own domain-separation prefix `ratatoskr.document.v1\0`.

### D2. The digest depends on the text blocks only

The content digest is SHA-256 over the canonical JSON of the headings and paragraphs, so neither the source address, the route (a Telegram blob or a fetched URL) nor the strategy changes it. Two routes to the same text give one identity. Different text for the same owner, or the same text for another owner, give another identity. A re-extraction whose text changed is a new document; versioning is out of scope.

### D3. A run does not carry an identity

`extraction_runs.document_id` is deleted (it was `unique`, which would now forbid the second run over the same bytes). The claim query already reads the run; it returns `owner_id` instead, and `QueuedRun.owner` feeds the parser inputs. Terminal updates match `run_id` and `status = 'running'`, which already identify the run.
