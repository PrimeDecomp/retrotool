// Run uniformly after the kernel and before discard. The caller must establish
// that helpers remain active in this backend's quad operations. Integer payloads
// preserve every fault bit and PC without floating-point conversion.
fn remax_quad_fault(status: u32, pc: u32) -> vec2<u32> {
    let local = vec2<u32>(status, select(4294967295u, pc, status != 0u));
    let a = quadBroadcast(local, 0u);
    let b = quadBroadcast(local, 1u);
    let c = quadBroadcast(local, 2u);
    let d = quadBroadcast(local, 3u);
    return vec2<u32>(a.x | b.x | c.x | d.x, min(min(a.y, b.y), min(c.y, d.y)));
}
