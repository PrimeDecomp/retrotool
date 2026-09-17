// Round a 48-bit nonzero significand held as (low, high). The caller chooses a
// shift whose quotient fits 24 bits; large shifts retain sticky information.
fn remax_round_product(product: vec2<u32>, shift: i32, mode: u32, negative: bool) -> u32 {
    if (shift <= 0) {
        return product.x << u32(-shift);
    }
    var whole = 0u;
    var guard = false;
    var sticky = false;
    let count = u32(shift);
    if (count < 32u) {
        whole = (product.x >> count) | (product.y << (32u - count));
        guard = ((product.x >> (count - 1u)) & 1u) != 0u;
        sticky = (product.x & ((1u << (count - 1u)) - 1u)) != 0u;
    } else if (count == 32u) {
        whole = product.y;
        guard = (product.x & 0x80000000u) != 0u;
        sticky = (product.x & 0x7fffffffu) != 0u;
    } else if (count < 64u) {
        whole = product.y >> (count - 32u);
        guard = ((product.y >> (count - 33u)) & 1u) != 0u;
        sticky = product.x != 0u || (product.y & ((1u << (count - 33u)) - 1u)) != 0u;
    } else {
        sticky = (product.x | product.y) != 0u;
    }
    let remainder = guard || sticky;
    let increment = (mode == 0u && guard && (sticky || (whole & 1u) != 0u)) ||
        (mode == 1u && negative && remainder) || (mode == 2u && !negative && remainder);
    return whole + select(0u, 1u, increment);
}

// Exact binary32 product scaled by a power of two before its sole rounding.
// Inputs have already passed through their native modifiers/FTZ. Integer
// arithmetic avoids intermediate overflow, underflow and host denormal policy.
// NaNs follow APFloat's comparison payload policy, not a claimed SM53 payload.
fn remax_scaled_multiply(a: u32, b: u32, scale: i32, mode: u32, ftz: bool) -> u32 {
    if (remax_nan(a)) {
        return a | 0x00400000u;
    }
    if (remax_nan(b)) {
        return b | 0x00400000u;
    }
    let sign = (a ^ b) & 0x80000000u;
    let negative = sign != 0u;
    let magnitude_a = a & 0x7fffffffu;
    let magnitude_b = b & 0x7fffffffu;
    if (magnitude_a == 0x7f800000u || magnitude_b == 0x7f800000u) {
        if (magnitude_a == 0u || magnitude_b == 0u) {
            return 0x7fc00000u;
        }
        return sign | 0x7f800000u;
    }
    if (magnitude_a == 0u || magnitude_b == 0u) {
        return sign;
    }
    let exponent_a = magnitude_a >> 23u;
    let exponent_b = magnitude_b >> 23u;
    let significand_a = (a & 0x7fffffu) | select(0u, 0x800000u, exponent_a != 0u);
    let significand_b = (b & 0x7fffffu) | select(0u, 0x800000u, exponent_b != 0u);

    // A 24-by-24-bit product needs four bounded 16-bit partial products.
    let low = (significand_a & 0xffffu) * (significand_b & 0xffffu);
    let middle = (significand_a >> 16u) * (significand_b & 0xffffu) +
        (significand_a & 0xffffu) * (significand_b >> 16u) + (low >> 16u);
    let high = (significand_a >> 16u) * (significand_b >> 16u) + (middle >> 16u);
    let product = vec2<u32>((middle << 16u) | (low & 0xffffu), high);
    var highest = remax_flo(product.x, false, false);
    if (high != 0u) {
        highest = 32u + remax_flo(high, false, false);
    }
    let power = i32(max(exponent_a, 1u)) + i32(max(exponent_b, 1u)) - 300 + scale;
    let exponent = power + i32(highest);

    // Native FMUL.FTZ flushes exact tiny products even if rounding would have
    // rescued the minimum normal number. This ordering is measured on SM75.
    if (ftz && exponent < -126) {
        return sign;
    }
    if (exponent > 127) {
        let infinity = mode == 0u || (mode == 1u && negative) || (mode == 2u && !negative);
        return sign | select(0x7f7fffffu, 0x7f800000u, infinity);
    }
    let shift = max(i32(highest) - 23, -power - 149);
    let significand = remax_round_product(product, shift, mode, negative);
    if (exponent < -126) {
        // A rounded carry naturally becomes the minimum normal encoding.
        return sign | significand;
    }
    // A rounded carry also advances a normal exponent, possibly to infinity.
    return sign | ((u32(exponent + 126) << 23u) + significand);
}
