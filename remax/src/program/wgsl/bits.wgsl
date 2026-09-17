fn remax_flush(bits: u32) -> u32 {
    return select(bits, bits & 0x80000000u, (bits & 0x7f800000u) == 0u);
}

fn remax_nan(bits: u32) -> bool {
    return (bits & 0x7fffffffu) > 0x7f800000u;
}

fn remax_finite(bits: u32) -> bool {
    return (bits & 0x7f800000u) != 0x7f800000u;
}

fn remax_saturate(bits: u32) -> u32 {
    return select(min(bits, 0x3f800000u), 0u, remax_nan(bits) || (bits & 0x80000000u) != 0u);
}

fn remax_order(bits: u32) -> u32 {
    return select(bits | 0x80000000u, ~bits, (bits & 0x80000000u) != 0u);
}

fn remax_equal(a: u32, b: u32) -> bool {
    return a == b || ((a | b) & 0x7fffffffu) == 0u;
}

// minimumNumber/maximumNumber. Integer selection retains signed zeros and
// subnormals independently of host f32 behavior. The both-NaN payload is the
// comparison model's SM75 measurement; it has not been measured on SM53.
fn remax_min_max(a: u32, b: u32, minimum: bool) -> u32 {
    if (remax_nan(a)) {
        return select(b, 0x7fffffffu, remax_nan(b));
    }
    if (remax_nan(b)) {
        return a;
    }
    let take_a = select(remax_order(a) > remax_order(b), remax_order(a) < remax_order(b), minimum);
    return select(b, a, take_a);
}

// IEEE rounding to an integral binary32 value. Preserve signed zeros and avoid
// f32 arithmetic, whose host implementation may flush subnormals. NaNs are
// quieted with their other bits preserved, as in the APFloat comparison model.
fn remax_round_integral(bits: u32, mode: u32) -> u32 {
    let sign = bits & 0x80000000u;
    let magnitude = bits & 0x7fffffffu;
    if (magnitude >= 0x4b000000u) {
        return select(bits, bits | 0x00400000u, remax_nan(bits));
    }
    if (magnitude < 0x3f800000u) {
        let one = magnitude != 0u && (
            (mode == 0u && magnitude > 0x3f000000u) ||
            (mode == 1u && sign != 0u) || (mode == 2u && sign == 0u));
        return sign | select(0u, 0x3f800000u, one);
    }
    let shift = 150u - (magnitude >> 23u);
    let unit = 1u << shift;
    let tail = magnitude & (unit - 1u);
    let whole = magnitude - tail;
    let halfway = unit >> 1u;
    let increment = tail != 0u && (
        (mode == 0u && (tail > halfway || (tail == halfway && (whole & unit) != 0u))) ||
        (mode == 1u && sign != 0u) || (mode == 2u && sign == 0u));
    return sign | (whole + select(0u, unit, increment));
}
// Total bit-field extraction: source bits outside the word are zero/sign padded.
// WGSL extractBits clamps its range; handle the entirely padded case separately.
fn remax_extract_bits(value: u32, offset: u32, count: u32, signed_source: bool) -> u32 {
    if (count == 0u) {
        return 0u;
    }
    if (offset >= 32u) {
        return select(0u, 0xffffffffu, signed_source && (value & 0x80000000u) != 0u);
    }
    let width = min(count, 32u - offset);
    if (signed_source) {
        return bitcast<u32>(extractBits(bitcast<i32>(value), offset, width));
    }
    return extractBits(value, offset, width);
}
