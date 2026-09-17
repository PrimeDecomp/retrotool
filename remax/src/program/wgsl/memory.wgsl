@group(__GROUP__) @binding(__BINDING__)
var<storage, read> remax_global_memory: array<u32>;

fn remax_address_le(a: vec2<u32>, b: vec2<u32>) -> bool {
    return a.y < b.y || (a.y == b.y && a.x <= b.x);
}

fn remax_global_byte(address: vec2<u32>, status: ptr<function, u32>) -> u32 {
    var low = 0u;
    var high = remax_global_memory[0];
    // Upper bound by base address; the preceding region is the only candidate.
    loop {
        if (low >= high) { break; }
        let mid = low + (high - low) / 2u;
        let entry = 1u + mid * 5u;
        let base = vec2<u32>(remax_global_memory[entry], remax_global_memory[entry + 1u]);
        if (remax_address_le(base, address)) { low = mid + 1u; } else { high = mid; }
    }
    if (low == 0u) { *status = 512u; return 0u; }
    let entry = 1u + (low - 1u) * 5u;
    let base = vec2<u32>(remax_global_memory[entry], remax_global_memory[entry + 1u]);
    let offset = address.x - base.x;
    let upper = address.y - base.y - select(0u, 1u, address.x < base.x);
    if (upper != 0u || offset >= remax_global_memory[entry + 2u]) {
        *status = 512u; return 0u;
    }
    let bitmap = remax_global_memory[entry + 4u];
    if (bitmap != 0u && (remax_global_memory[bitmap + offset / 32u] & (1u << (offset % 32u))) == 0u) {
        *status = 1024u; return 0u;
    }
    return (remax_global_memory[remax_global_memory[entry + 3u] + offset / 4u] >> (offset % 4u * 8u)) & 255u;
}

fn remax_global_load(address: vec2<u32>, bytes: u32, status: ptr<function, u32>) -> vec4<u32> {
    if ((address.x & (bytes - 1u)) != 0u) { *status = 4u; return vec4<u32>(0u); }
    if (address.y == 4294967295u && bytes - 1u > 4294967295u - address.x) {
        *status = 512u; return vec4<u32>(0u);
    }
    var result = vec4<u32>(0u);
    for (var i = 0u; i < bytes; i += 1u) {
        let low = address.x + i;
        let high = address.y + select(0u, 1u, low < address.x);
        let byte = remax_global_byte(vec2<u32>(low, high), status);
        if (*status != 0u) { return vec4<u32>(0u); }
        result[i / 4u] |= byte << (i % 4u * 8u);
    }
    return result;
}
