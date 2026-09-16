# From ROOM definitions to a scene

## Loading and activation

A ROOM contains definitions, not a snapshot of all currently active objects.
Loading eligibility, component construction and effective activity are separate
states. This distinction affects geometry, RoomController residency and lighting.

Load units list resources and layers. Layer headers also list load-unit
memberships and a reload-set ID. Empty memberships have special loading rules;
the nil unit's association cannot be reconstructed by treating every list as
an ordinary GUID join.

Startup requests combine authored defaults, saved environment values and prefix
conditions. Variables search the current area then loaded parents; a stored zero
stops the search. AND/OR do not short-circuit. Evaluation stops at the first
completed root; empty or incomplete programs return true. A parser still needs
to retain the complete record.

Performance groups issue ordered layer requests. Layer activity depends on a
nonzero signed request count and the area's active state. Reset-on-activation
layers can construct components on request and queue deletion on release;
queued deletion does not immediately remove an object. Callback ordering and
duplicate requests matter. There is no recovered LHED boolean that simply
means “this layer is active.”

At the component level, effective activity requires four gates: its own flag,
its owning Entity, its layer and its template. An Entity's authored inactive
flag remains inactive even when construction requests activity. Eligibility to
load a layer is not enough to enable every component or schedule its updates.

The original startup evaluator is `0x71003fb2d8`; layer request consumers begin
at `0x710047728c` and `0x7100477828`; component activity is tested at
`0x71005c3d8c`. See [load units](../lib/src/format/script_load_units.rs),
[conditions](../lib/src/format/load_unit_logic.rs),
[performance groups](../lib/src/format/script_performance_groups.rs), and
[activation](../lib/src/format/script_activation.rs).

## Ownership and generated definitions

Entity `COMP` links describe component ownership. The same link ID also appears
on TemplateManager records, where it must not be interpreted as Entity ownership.
Generated groups can reuse editor GUIDs from ordinary definitions. Resolve a
generated batch locally first, then fall back to ordinary definitions in the
ROOM. Merging all definitions into one global GUID map loses this distinction.

Construction assigns versioned runtime IDs and owning-Entity pointers. Generated
target remapping chooses the first matching object in the new batch, preserving
order. Base component Create then prunes links according to their retention
flag. Parsers must retain the authored links because earlier construction steps
can still use them.

Sources: base Create (`0x710046de20`), Entity component caching (`0x710046a3dc`)
and generated remapping (`0x710047c140`); see [ROOM](../lib/src/format/room.rs)
and [script layers](../lib/src/format/script_layers.rs).

## Links and runtime identity

Editor GUIDs can resolve to multiple live instances. Runtime IDs are versioned:
their low 16 bits identify a slot, but lookup checks the full ID. `0xffffffff`
is invalid; zero is valid. Some lookups include objects in limbo, while ordinary
linked-object iteration also tests registration state.

Load-time target caching selects the first GUID alias without validating that
the object is live. A stale first alias does not imply trying later aliases.
Later link traversal preserves authored order and duplicates, and applies
generated-instance communication rules. Entity targets can expand to their
components, but the native iterator has one expansion cursor rather than a
recursive stack; nested Entities replace that cursor.

Consequently, a generic recursive GUID walk is not equivalent to script dispatch.
For baked color, ColorModifier mode 1 resolves linked components and then checks
their concrete RoomSettings type before changing an area's color. Matching the
interface alone is insufficient.

`CLinkedObjectRange::next_item` (`0x710047b75c`),
`CLinkedComponentRange::next_item` (`0x710047bb2c`) and color dispatch
(`0x7100895b10`) establish this path. [Link resolution](../lib/src/format/script_links.rs)
accepts explicit runtime snapshots. Automatic scene construction still needs
object factories, registration, message processing and the game's startup order;
the recovered helpers do not by themselves simulate a running ROOM.
