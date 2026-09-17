# remax

Maxwell shader decoding and translation in Rust. Game containers, NVN state and
renderer integration belong to the caller; the crate has no graphics API or
game-format dependencies.

`program` lifts supported instructions into verified semantic SSA, with an
interpreter and WGSL emission. `maxwell::graphics` links SPHv3 vertex/fragment
pairs into stage modules with explicit resource, interpolation and fault
contracts. The viewer offers this compiler experimentally alongside the older
`wgsl` translator. IR internals remain private.

AL2P attribute selection stays dynamic. Opaque address/predicate provenance
survives copies and compatible joins without fabricating native bits. Integer
analysis narrows the input interface; availability is checked when a selected
component is read. RRO/MUFU pairs likewise retain opaque intermediates.
In acyclic functions, consumer path facts can recover a unique dominating
producer before demand removes unobservable join/select alternatives. Accesses,
execution points and opaque type checks are preserved.

`program::wgsl::Profile::Checked` diagnoses executed accesses. `Rendering` moves
proven fixed constant reads into `Kernel::constant_preconditions`; call `bind`
for every upload. Both profiles keep unresolved checks and fault reporting.
Neither specializes uniform values. Missing inputs remain unavailable.
Rendering also promotes proven local storage into SSA, removes redundant input
flushing, reuses identical dominating scalar expressions, merges linear blocks
and emits supported natural loops directly. Reuse preserves operand order and
numeric controls; memory reads, interpolation and texture operations stay in place.
Execution checkpoints are preserved; Checked retains the original graph, memory
path and cyclic dispatcher for comparison.
Rendering charges proven finite prefixes together and removes the counter when
a whole-function bound fits the budget, including recognized bit-clearing loops.
Potential exhaustion retains individual checks; Checked keeps cyclic accounting.

Arithmetic and special-function approximations require explicit opt-in. Closed
SSY/SYNC regions are supported; general GPU reconvergence remains incomplete.
Implicit texture sampling is restricted to an established entry path and needs
helper-inclusive quad fault reporting. The interpreter, host filtering and
emulator comparisons do not establish bit-exact SM53 behavior.

Fragment demotion preserves helper execution and suppresses exports. The
interpreter returns `Execution::Discarded`; WGSL carries that disposition to
the graphics wrapper, which reports faults before discarding. This lowering
must be revisited before adding guest external writes, atomics or helper queries.

The compiler tests have a separate Cargo workspace:

```sh
cargo test --manifest-path remax/Cargo.toml
cargo test --manifest-path remax/Cargo.toml ir::tests::wgsl -- --ignored --nocapture
```

The second command requires a native GPU. [Material rendering tests](../render-tests/)
exercise real NVN bindings and independently generated emulator outputs; no
emulator is a production dependency. [Compatibility notes](../docs/rendering-compatibility.md)
track dependency workarounds and their removal conditions.
