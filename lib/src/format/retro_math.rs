//! Recovered MPR CPU math. This is separate from Maxwell shader arithmetic.

// FRSQRTE output mantissa bins, captured from the original instruction at
//7100065f8c. First half: odd biased exponent; second half: even. Low16 input
// fraction bits do not affect the estimate. Table stores only eight result bits.
const RSQRT_ESTIMATE: [u8; 256] = [
    255, 253, 251, 249, 247, 245, 243, 242, 240, 238, 236, 234, 233, 231, 229, 228, 226, 224, 223,
    221, 219, 218, 216, 215, 213, 212, 210, 209, 207, 206, 204, 203, 201, 200, 198, 197, 196, 194,
    193, 192, 190, 189, 188, 186, 185, 184, 183, 181, 180, 179, 178, 176, 175, 174, 173, 172, 170,
    169, 168, 167, 166, 165, 164, 163, 162, 160, 159, 158, 157, 156, 155, 154, 153, 152, 151, 150,
    149, 148, 147, 146, 145, 144, 143, 142, 141, 140, 140, 139, 138, 137, 136, 135, 134, 133, 132,
    131, 131, 130, 129, 128, 127, 126, 126, 125, 124, 123, 122, 121, 121, 120, 119, 118, 118, 117,
    116, 115, 114, 114, 113, 112, 111, 111, 110, 109, 109, 108, 107, 106, 105, 104, 103, 101, 100,
    99, 97, 96, 95, 93, 92, 91, 90, 88, 87, 86, 85, 84, 82, 81, 80, 79, 78, 77, 76, 75, 74, 72, 71,
    70, 69, 68, 67, 66, 65, 64, 63, 62, 61, 60, 60, 59, 58, 57, 56, 55, 54, 53, 52, 51, 51, 50, 49,
    48, 47, 46, 46, 45, 44, 43, 42, 42, 41, 40, 39, 38, 38, 37, 36, 35, 35, 34, 33, 33, 32, 31, 30,
    30, 29, 28, 28, 27, 26, 26, 25, 24, 24, 23, 22, 22, 21, 20, 20, 19, 19, 18, 17, 17, 16, 16, 15,
    14, 14, 13, 13, 12, 11, 11, 10, 10, 9, 9, 8, 8, 7, 6, 6, 5, 5, 4, 4, 3, 3, 2, 2, 1, 1, 0,
];

/// CMath::SqrtF7100065f70, for finite inputs with default ARM FPCR rounding and
/// gradual underflow. Nonpositive input returns +0. Positive input is clamped to
/// the original 1e-35 minimum for the reciprocal estimate only, then two native
/// reciprocal-square-root refinement steps precede the final input multiply.
/// This deliberately differs from the host's correctly rounded square root.
pub fn sqrt_f(value: f32) -> f32 {
    if value <= 0.0 {
        return 0.0;
    }
    if !value.is_finite() {
        return f32::NAN;
    }
    let input = value.max(f32::from_bits(0x0554_ad2e));
    value * reciprocal_sqrt(input)
}

/// Two ARM reciprocal-square-root refinement steps, shared by native vector math.
fn reciprocal_sqrt(input: f32) -> f32 {
    // FRSQRTE(+inf) is zero; FRSQRTS(+inf, 0) returns 1.5, retaining zero
    // through both refinements. Finite vector components can overflow the dot.
    if input == f32::INFINITY {
        return 0.0;
    }
    let bits = input.to_bits();
    let exponent = (bits >> 23) & 255;
    let index = ((bits >> 16) & 127) + if exponent & 1 == 0 { 128 } else { 0 };
    let mut estimate = f32::from_bits(
        ((380 - exponent) / 2) << 23 | u32::from(RSQRT_ESTIMATE[index as usize]) << 15,
    );
    for _ in 0..2 {
        let square = estimate * estimate;
        // FRSQRTS rounds (3 - input*square)/2 once. Double precision represents
        // this finite near-unit product/subtraction exactly before the f32 cast.
        let correction = ((3.0f64 - f64::from(input) * f64::from(square)) * 0.5) as f32;
        estimate *= correction;
    }
    estimate
}

/// CVector3f::AsNormalizedOrZero (710006b6b8). Preserve its finite/length checks
/// and SIMD reduction order, which differ from a generic vector normalize.
pub fn normalized_or_zero(value: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = value;
    if !value.iter().all(|v| v.is_finite()) || (x * x + y * y) + z * z < 9.999_999_4e-29 {
        return [0.0; 3];
    }
    let scale = reciprocal_sqrt((x * x + z * z) + (y * y + 0.0));
    value.map(|v| v * scale)
}

/// CMath::FastCosR (7100069280), used for decal angle thresholds. This is the
/// engine's polynomial and reduction order, not the host cosine function.
pub fn fast_cos_r(mut angle: f32) -> f32 {
    if angle.abs() > std::f32::consts::PI {
        let turns = (angle * (std::f32::consts::FRAC_1_PI * 0.5)) as i32;
        let reduced = angle + turns as f32 * -std::f32::consts::TAU;
        angle = if reduced >= std::f32::consts::PI {
            reduced - std::f32::consts::TAU
        } else if reduced <= -std::f32::consts::PI {
            reduced + std::f32::consts::TAU
        } else {
            reduced
        };
    }
    let square = angle * angle;
    let cube = square * square * square;
    square * -0.499_980_03
        + 1.0
        + square * square * 0.041_620_344
        + cube * -0.001_363_610_3
        + square * cube * 2.016_943_5e-5
}

/// CRandom16::RangeF71000b9f90. The caller owns the actual 32-bit stream state.
/// Float generation uses the low28 bits, rounded to f32 before scaling. The
/// resulting fraction can round to1; do not replace it with a 16-bit fraction.
pub fn random_range_f(state: &mut u32, min: f32, max: f32) -> f32 {
    *state = state.wrapping_mul(0x41c6_4e6d).wrapping_add(0x3039);
    let fraction = (*state & 0x0fff_ffff) as f32 * f32::from_bits(0x3180_0000);
    (max - min) * fraction + min
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_decal_angle_thresholds_preserve_polynomial_and_reduction() {
        // Results captured by executing CMath::FastCosR in the original ARM64 image.
        for (input, expected) in [
            (0x3f49_0fdb, 0x3f35_04f2),
            (0xc0a5_dc67, 0x3ee8_3913),
            (0x4073_184c, 0xbf4a_c2d0),
            (0xc0a4_a250, 0x3ed6_8f98),
            (0x40a1_6b8d, 0x3ea6_de39),
            (0x40be_7004, 0x3f72_0518),
        ] {
            assert_eq!(fast_cos_r(f32::from_bits(input)).to_bits(), expected, "angle {input:x}");
        }
    }

    #[test]
    fn original_vector_normalization_preserves_rounding_and_degenerate_cases() {
        // Direct results from CVector3f::AsNormalizedOrZero in the ARM64 image.
        for (input, expected) in [
            ([0x3f80_0000, 0x4000_0000, 0x4040_0000], [0x3e88_d677, 0x3f08_d677, 0x3f4d_41b2]),
            ([0xc040_0000, 0x4080_0000, 0xc000_0000], [0xbf0e_9d30, 0x3f3e_26eb, 0xbebe_26eb]),
            ([0x2690_1d7d, 0, 0], [0, 0, 0]),
            ([0x2834_24dc, 0, 0], [0x3f80_0001, 0, 0]),
            ([0x7149_f2ca, 0xf1c9_f2ca, 0x7217_7617], [0, 0x8000_0000, 0]),
        ] {
            assert_eq!(normalized_or_zero(input.map(f32::from_bits)).map(f32::to_bits), expected);
        }
        for input in [[0.0; 3], [f32::INFINITY, 0.0, 0.0], [f32::NAN, 1.0, 2.0]] {
            assert_eq!(normalized_or_zero(input).map(f32::to_bits), [0; 3]);
        }
    }

    #[test]
    fn original_math_preserves_estimate_rounding_and_rng_upper_endpoint() {
        for (input, expected) in [
            (0x00000000u32, 0x00000000u32),
            (0x00000001u32, 0x120c6eefu32),
            (0x007fffffu32, 0x1d8c6eeeu32),
            (0x80000000u32, 0x00000000u32),
            (0xbf800000u32, 0x00000000u32),
            (0x3f800000u32, 0x3f800000u32),
        ] {
            assert_eq!(sqrt_f(f32::from_bits(input)).to_bits(), expected);
        }
        let mut seed = 0x5dbd_bb1e;
        assert_eq!(random_range_f(&mut seed, 0.0, 1.0), 1.0);
        assert_eq!(seed, 0x0fff_ffff);
    }
}
