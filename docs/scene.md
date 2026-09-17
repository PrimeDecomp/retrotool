# From ROOM definitions to a scene

## Model containers

MCON has two placement paths. Ordinary visuals select shared CMDL resources
and modulation colors by index; table order does not imply grouping by model.
Both ordinary and instanced native draws use the selected color. The deferred
pass also adds per-instance G-buffer flags.

World-model placements instead contain a WMDL resource ID and a transform.
Their resource table supplies preload tokens; it is not the placement list.
The ModCon component creates a separate world-model scene node for each
placement (`BuildWorldModels`, `0x71007a55b8`). These nodes also follow the
component's effective activity and scene-visibility gate.

The engine composes the container's parent transform with each visual transform
before computing world bounds and mirroring. Native
`CModConRenderData::Allocate` (`0x71001be62c`), `WriteBaseInstancingComponents`
(`0x71001c0144`) and `RenderUnsortedEntryConcurrent` (`0x71001b5bcc`) establish
these relationships. See the [MCON parser](../lib/src/format/mcon.rs).

Visibility uses the world-aligned box enclosing each transformed model's authored
bounds. Recomputing a box from individual meshes, or testing the transformed local
box directly, can reject draws the native owner retains. MCON does not add the
dynamic padding offered by WIND; [wind ownership](animation.md#wind) differs for
world-model renderables.

The two placement paths have separate atlas-lookup arrays, indexed by placement
order. An empty array disables atlas lookup; an invalid individual lookup also
disables it for that placement. Sharing a model resource does not imply sharing
its baked UV transform. Atlas textures and lighting context come from the area.

The ROOM's ModCon component references the container resource. Its native
`EntityLoaded` (`0x71007a5168`) takes the owning area's transform, rather than
the Entity's authored transform, and passes it to ordinary visuals, world-model
placements and collision construction. A ROOM-local inspection can use an
identity area transform; assembling several rooms requires their area placement.
The surrounding ROOM supplies loading and activity state, and MCON's remaining
spatial tables control queries beyond these geometry relationships.

MCDD adds geometry decals independently of the visual meshes. Its shared IDs
refer to MATI material instances. The owner composes each decal's placement
with the container transform, extracts the axis scales and halves them for
projection extents. The decal manager generates geometry from eligible scene
surfaces through a stream-out pass, then draws it with the selected material
and sort order. Loading the placements therefore requires both MATI support
and receiver-geometry generation to reproduce their appearance; a textured
quad is insufficient. `BuildModConDecals` (`0x71007a61d4`) and
`submit_new_stream_out_requests` (`0x7100203b90`) establish this path.
The [decal parser](../lib/src/format/mcon/decal.rs) preserves the remaining
fields whose roles are unresolved.

The static generator tests the receiver triangle's angle and rejects a triangle
only when all three vertices lie outside a common box plane. Accepted triangles
keep their original vertices; generation does not clip or subdivide them. It
adds projected UVs and six signed plane distances. The final decal vertex shader
exports those distances to hardware clip planes, which perform the box clipping.
The stream-out layout reuses bone-weight and bone-index semantics for these
distances; they do not describe skinning.

Receiver draws use mesh-index group zero rather than the camera-selected LOD.
MCON receivers test visual-instance world bounds; world-model receivers also
test individual mesh bounds. Scene registration determines which owners enter
the query. The recovered [projection and generation](../lib/src/format/decal.rs)
implement the static shader's geometry operation; receiver registration and the
final material draw remain separate integration requirements.

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
