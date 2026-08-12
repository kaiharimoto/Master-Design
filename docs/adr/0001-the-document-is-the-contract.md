# 1. The document is the contract

Accepted.

## Context

Two editors change the same design: a person, and an AI coding model. The common
arrangement — the model writes code, the person edits the code — breaks at the first
handoff. Once someone touches generated output, the model can only understand what
changed by re-reading and re-inferring its own code, and whatever structure it had in
mind is gone. Every pass degrades.

## Decision

Neither editor writes to the document directly. Both submit **operations** which are
validated, applied atomically, and recorded with their inverses on a single shared undo
stack.

Operations apply to a *copy* of the document which is swapped in on success. Undo stores
inverse operations rather than snapshots.

## Consequences

Good:

- An AI edit is undone with the same keystroke as a mouse drag. Without this nobody would
  let a model touch anything, so it is the load-bearing property.
- One validator. A model cannot write a node the GUI could not have written.
- A patch that fails half-way leaves the document untouched rather than half-edited.
- The operation list is a record of intent, not a diff of bytes.

Costs:

- Every change is a round trip in the studio, since the backend owns the document. Drags
  work around this by being optimistic and committing once on release (ADR 5).
- Copying the document per patch is wasted work on a large document. It buys correctness
  that rollback machinery would otherwise have to earn, and documents are small enough
  that it has not been measurable.
- Inverse operations are cheap in memory but mean an operation without a correct inverse
  is a silent corruption. Every operation has a test asserting its inverse restores the
  document byte for byte.

## Alternatives considered

**Snapshots for undo.** Simpler and impossible to get subtly wrong. A hundred steps deep
on a large document is hundreds of megabytes, which is not available on the phone this
has to run on.

**CRDTs.** Would give real concurrent editing rather than last-write-wins. Enormous
complexity for a problem M1 does not have — one person and one agent, coordinating
through a file.
