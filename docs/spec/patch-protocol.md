# The patch protocol

Every change to a document is a list of operations. Nothing mutates a document any other
way — not the GUI, not the CLI, not the AI.

A list applies as one undoable step, and applies **atomically**: operations run against a
copy which is swapped in on success, so a failure part-way leaves the document exactly as
it was.

## Operations

### node.insert

```json
{ "op": "node.insert", "parent": "nd_root", "index": 2, "node": {} }
```

`index` appends when omitted. Refuses a parent that cannot hold children, an index past the
end, or a subtree containing an id already in the document.

### node.delete

```json
{ "op": "node.delete", "id": "nd_card" }
```

Takes the whole subtree. Refuses the page root.

### node.update

```json
{ "op": "node.update", "id": "nd_card", "path": "fills.0.color", "value": "#ff0055" }
```

The workhorse. `path` is dotted, with numeric segments indexing arrays. A `null` value
clears an optional property.

Intermediate containers are **never created**. A typo like `fil1s.0.color` is an error
rather than a stray key that silently does nothing.

The result is round-tripped through the typed model, so a bad colour, an unknown enum or a
wrong-shaped value fails here rather than reaching a file. Ids cannot be changed.

### node.move

```json
{ "op": "node.move", "id": "nd_card", "parent": "nd_section", "index": 0 }
```

Reparents and reorders; also how "bring to front" is expressed. Refuses a move into the
node's own descendant, which would detach the subtree from the document.

### path.boolean

```json
{ "op": "path.boolean", "ids": ["nd_a", "nd_b"], "mode": "subtract", "resultId": "nd_c" }
```

`union` | `subtract` | `intersect` | `exclude`. Operands must share a parent — without a
common coordinate space there is no single answer to where the result belongs. Each
operand's transform is applied before combining, and the first operand's appearance, name
and stacking position carry over.

Supplying `resultId` makes the operation reproducible, which matters for tests and for a
model that wants to refer to the result in a later operation of the same patch.

### Pages, timelines, tokens

```json
{ "op": "page.insert",     "page": {}, "index": 1 }
{ "op": "page.delete",     "page": "about" }
{ "op": "page.update",     "page": "index", "path": "name", "value": "Landing" }
{ "op": "timeline.set",    "page": "index", "timeline": {} }
{ "op": "timeline.remove", "page": "index", "id": "tl_hero" }
{ "op": "tokens.set",      "path": "colors.accent", "value": "#ff0055" }
```

`page.update` refuses paths under `root` — the scene graph is edited with `node.*`
operations, not smuggled through a page property.

`tokens.set` will create a missing group, since the shapes are fixed and known and asking a
model to create the container first is friction with no upside.

## Inverses

Applying an operation returns the operations that undo it, already ordered for replay. That
is what fills the undo stack, and it is why a change made by an AI is undone with the same
keystroke as a mouse drag.

Every operation has a test asserting its inverse restores the document **byte for byte**.
An operation without a correct inverse is a silent corruption, so this is the property the
suite guards most closely.

## Selectors

Operations address ids; the tools that produce operations mostly address selectors.

```
#nd_01hq5...          a specific node
@card                 every node tagged with the role "card"
type:text             every text node
name:"Hero title"     by layer name
*                     everything
@section @card        cards anywhere inside a section        (descendant)
type:path@decorative  paths that are also decorative         (compound)
@card, @cta           either                                 (union, document order)
```

Deliberately CSS-shaped, because the people and the models using it already know CSS.
Results come back in document order, without duplicates.
