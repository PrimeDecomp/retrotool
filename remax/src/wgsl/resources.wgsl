// Translator ABI: 32 (word offset, tagged byte length) pairs then bank data.
// Length bit31 adds a per-byte validity bitmap after the padded bank data.
@group(1) @binding(__BINDING__) var<storage, read> mpr_constant_memory: array<u32>;

// Fault bits: 1 invalid/absent bank, 2 out of range, 4 unaligned, 8 unknown bytes.
// Failed reads return zero and set a caller-visible sticky flag. This is a
// declared host fault policy, not claimed Maxwell behavior for invalid loads.
fn mpr_ldc(bank: u32, address: u32, size: u32, status: ptr<function, u32>) -> vec2<u32> {
    return mpr_ldc_impl(bank, address, size, true, true, status);
}

// The compiler plan may discharge layout checks through the typed snapshot
// contract, and alignment checks through integer analysis. Bank presence,
// dynamic bounds and byte validity remain execution-dependent obligations.
fn mpr_ldc_impl(
    bank: u32,
    address: u32,
    size: u32,
    check_layout: bool,
    check_alignment: bool,
    status: ptr<function, u32>,
) -> vec2<u32> {
    let count = arrayLength(&mpr_constant_memory);
    if (check_layout && (count < 64u || bank >= 32u)) {
        *status |= 1u;
        return vec2<u32>(0u);
    }
    let start = mpr_constant_memory[bank * 2u];
    let tagged_length = mpr_constant_memory[bank * 2u + 1u];
    let length = tagged_length & 0x7fffffffu;
    if (length == 0u) {
        *status |= 1u;
        return vec2<u32>(0u);
    }
    if (check_layout) {
        if (length > 65536u || start < 64u || start > count) {
            *status |= 1u;
            return vec2<u32>(0u);
        }
        if ((length + 3u) / 4u > count - start) {
            *status |= 1u;
            return vec2<u32>(0u);
        }
    }
    if (address > length || size > length - address) {
        *status |= 2u;
        return vec2<u32>(0u);
    }
    if (check_alignment && (address & (size - 1u)) != 0u) {
        *status |= 4u;
        return vec2<u32>(0u);
    }
    if ((tagged_length & 0x80000000u) != 0u) {
        let bitmap = start + (length + 3u) / 4u;
        if (check_layout && (length + 31u) / 32u > count - bitmap) {
            *status |= 1u;
            return vec2<u32>(0u);
        }
        for (var byte = address; byte < address + size; byte = byte + 1u) {
            if ((mpr_constant_memory[bitmap + byte / 32u] & (1u << (byte & 31u))) == 0u) {
                *status |= 8u;
                return vec2<u32>(0u);
            }
        }
    }
    let word = start + address / 4u;
    var result = vec2<u32>(mpr_constant_memory[word], 0u);
    if (size == 8u) {
        result.y = mpr_constant_memory[word + 1u];
    } else if (size < 4u) {
        result.x = (result.x >> ((address & 3u) * 8u)) & ((1u << (size * 8u)) - 1u);
    }
    return result;
}
