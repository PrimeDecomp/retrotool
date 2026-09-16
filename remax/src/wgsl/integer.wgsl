// Integer-only conversion avoids host float casts, subnormal flushing and
// undefined out-of-range conversions. Rounding: 0 nearest-even,1 floor,2 ceil,3 zero.
fn mpr_round_up(negative: bool, rounding: u32, remainder: u32, midpoint: u32, odd: bool) -> bool {
    if (remainder == 0u) { return false; }
    if (rounding == 0u) { return remainder > midpoint || (remainder == midpoint && odd); }
    return (rounding == 1u && negative) || (rounding == 2u && !negative);
}
fn mpr_f2i(bits: u32, signed_result: bool, rounding: u32, nan_result: u32) -> u32 {
    let magnitude = bits & 0x7fffffffu;
    if (magnitude > 0x7f800000u) { return nan_result; }
    let negative = (bits & 0x80000000u) != 0u;
    if (negative && !signed_result) { return 0u; }
    let limit = select(0xffffffffu, select(0x7fffffffu, 0x80000000u, negative), signed_result);
    let exponent = (bits >> 23u) & 255u;
    var integer = 0u;
    if (exponent >= 159u) { integer = limit; }
    else if (exponent >= 150u) {
        integer = min(((bits & 0x7fffffu) | 0x800000u) << (exponent - 150u), limit);
    } else if (exponent >= 127u) {
        let shift = 150u - exponent;
        let significand = (bits & 0x7fffffu) | 0x800000u;
        integer = significand >> shift;
        let remainder = significand & ((1u << shift) - 1u);
        if (mpr_round_up(negative, rounding, remainder, 1u << (shift - 1u), (integer & 1u) != 0u)) { integer += 1u; }
    } else if (magnitude != 0u) {
        // Magnitudes below one, including subnormals. Compare IEEE bits at 0.5.
        if ((rounding == 0u && magnitude > 0x3f000000u) ||
            (rounding == 1u && negative) || (rounding == 2u && !negative)) { integer = 1u; }
    }
    return select(integer, 0u - integer, negative);
}
fn mpr_i2f(bits: u32, signed_source: bool, rounding: u32) -> u32 {
    let negative = signed_source && (bits & 0x80000000u) != 0u;
    let magnitude = select(bits, 0u - bits, negative);
    if (magnitude == 0u) { return 0u; }
    var highest = 0u;
    var remaining = magnitude;
    loop {
        remaining >>= 1u;
        if (remaining == 0u) { break; }
        highest += 1u;
    }
    var significand = 0u;
    if (highest <= 23u) { significand = magnitude << (23u - highest); }
    else {
        let shift = highest - 23u;
        significand = magnitude >> shift;
        let remainder = magnitude & ((1u << shift) - 1u);
        if (mpr_round_up(negative, rounding, remainder, 1u << (shift - 1u), (significand & 1u) != 0u)) { significand += 1u; }
    }
    // Addition deliberately carries a rounded 24-bit significand into exponent.
    return select(0u, 0x80000000u, negative) | (((highest + 126u) << 23u) + significand);
}

// Range bytes: low byte is offset, next byte is length. PTX bfe rules;
// uam/NAK encode this pair as the SASS range operand. Upper bytes are ignored.
fn mpr_bfe(base:u32, range:u32, signed_result:bool)->u32 {
    let offset=range&255u;
    let length=(range>>8u)&255u;
    if (length==0u) { return 0u; }
    let sign=signed_result && ((base>>min(offset + length - 1u,31u))&1u)!=0u;
    let padding=select(0u,0xffffffffu,sign);
    if (offset>=32u) { return padding; }
    let count=min(length,32u - offset);
    let mask=select((1u<<(count&31u)) - 1u,0xffffffffu,count==32u);
    return ((base>>offset)&mask)|(padding&~mask);
}
// Mesa NAK OpFlo folding + hardware tests: .SH returns leading count (32
// for zero/all sign bits); ordinary FLO returns 31-count, wrapping to ffffffff.
fn mpr_flo(raw:u32, signed_source:bool, shift_amount:bool)->u32 {
    let bits=select(raw,~raw,signed_source && (raw&0x80000000u)!=0u);
    // Naga 0.11 predates countLeadingZeros; its Metal firstLeadingBit lowering
    // returns clz instead of the bit index. Use bounded integer steps on all APIs.
    if (bits==0u) { return select(0xffffffffu,32u,shift_amount); }
    var remaining=bits;
    var leading=0u;
    if (remaining<0x10000u) { leading+=16u; remaining<<=16u; }
    if (remaining<0x1000000u) { leading+=8u; remaining<<=8u; }
    if (remaining<0x10000000u) { leading+=4u; remaining<<=4u; }
    if (remaining<0x40000000u) { leading+=2u; remaining<<=2u; }
    if (remaining<0x80000000u) { leading+=1u; }
    return select(31u - leading,leading,shift_amount);
}
