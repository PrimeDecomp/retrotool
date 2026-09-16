# remax

Maxwell shader decoding and translation in Rust. Game containers, NVN state and
renderer integration belong to the caller. No graphics API or game-format
dependencies are required.

The crate currently contains retrotool's existing decoder and WGSL translator.
The viewer and CLI use it through retrolib's NVN adapters. Generated entry-point
names and resource layouts retain the existing ABI during extraction.

The planned compiler separates semantic SSA, GPU execution analysis, structured
shader IR and WGSL emission. Its design draws on Mesa NIR, Switch recompilers and
Cranelift. Compiler internals will remain private while these phases develop;
the current register-based API is provisional. Unsupported behavior remains an
error, and successful translation still requires callers to satisfy the reported
resource requirements and specialization assumptions.

Switch recompilers will be temporary differential comparison targets alongside
hardware probes and small executable cases. They are not runtime dependencies.
