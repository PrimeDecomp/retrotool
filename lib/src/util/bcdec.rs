#![allow(unused_assignments, clippy::missing_safety_doc)]
/// bcdec from https://github.com/iOrange/bcdec, converted using c2rust
/// Original header comment and license included below

/* bcdec.h - v0.96
   provides functions to decompress blocks of BC compressed images
   written by Sergii "iOrange" Kudlai in 2022

   This library does not allocate memory and is trying to use as less stack as possible

   The library was never optimized specifically for speed but for the overall size
   it has zero external dependencies and is not using any runtime functions

   Supported BC formats:
   BC1 (also known as DXT1) + it's "binary alpha" variant BC1A (DXT1A)
   BC2 (also known as DXT3)
   BC3 (also known as DXT5)
   BC4 (also known as ATI1N)
   BC5 (also known as ATI2N)
   BC6H (HDR format)
   BC7

   BC1/BC2/BC3/BC7 are expected to decompress into 4*4 RGBA blocks 8bit per component (32bit pixel)
   BC4/BC5 are expected to decompress into 4*4 R/RG blocks 8bit per component (8bit and 16bit pixel)
   BC6H is expected to decompress into 4*4 RGB blocks of either 32bit float or 16bit "half" per
   component (96bit or 48bit pixel)

   For more info, issues and suggestions please visit https://github.com/iOrange/bcdec

   CREDITS:
      Aras Pranckevicius (@aras-p)      - BC1/BC3 decoders optimizations (up to 3x the speed)
                                        - BC6H/BC7 bits pulling routines optimizations
                                        - optimized BC6H by moving unquantize out of the loop
                                        - Split BC6H decompression function into 'half' and
                                          'float' variants

   bugfixes:
      @linkmauve

   LICENSE: See end of file for license information.
*/

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Bitstream {
    pub low: u64,
    pub high: u64,
}

pub const BC1_BLOCK_SIZE: usize = 8;
pub const BC2_BLOCK_SIZE: usize = 16;
pub const BC3_BLOCK_SIZE: usize = 16;
pub const BC4_BLOCK_SIZE: usize = 8;
pub const BC5_BLOCK_SIZE: usize = 16;
pub const BC6H_BLOCK_SIZE: usize = 16;
pub const BC7_BLOCK_SIZE: usize = 16;

unsafe fn color_block(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
    only_opaque_mode: bool,
) {
    let c0 = *(compressed_block as *const u16).add(0) as u32;
    let r0 = ((c0 >> 11 & 0x1f) * 527 + 23) >> 6;
    let g0 = ((c0 >> 5 & 0x3f) * 259 + 33) >> 6;
    let b0 = ((c0 & 0x1f) * 527 + 23) >> 6;

    let c1 = *(compressed_block as *const u16).add(1) as u32;
    let r1 = ((c1 >> 11 & 0x1f) * 527 + 23) >> 6;
    let g1 = ((c1 >> 5 & 0x3f) * 259 + 33) >> 6;
    let b1 = ((c1 & 0x1f) * 527 + 23) >> 6;

    let mut ref_colors: [u32; 4] =
        [0xff000000 | b0 << 16 | g0 << 8 | r0, 0xff000000 | b1 << 16 | g1 << 8 | r1, 0, 0];

    if c0 > c1 || only_opaque_mode {
        let r = 2u32.wrapping_mul(r0).wrapping_add(r1).wrapping_add(1).wrapping_div(3);
        let g = 2u32.wrapping_mul(g0).wrapping_add(g1).wrapping_add(1).wrapping_div(3);
        let b = 2u32.wrapping_mul(b0).wrapping_add(b1).wrapping_add(1).wrapping_div(3);
        ref_colors[2] = 0xff000000 | b << 16 | g << 8 | r;
        let r = r0.wrapping_add(2u32.wrapping_mul(r1)).wrapping_add(1).wrapping_div(3);
        let g = g0.wrapping_add(2u32.wrapping_mul(g1)).wrapping_add(1).wrapping_div(3);
        let b = b0.wrapping_add(2u32.wrapping_mul(b1)).wrapping_add(1).wrapping_div(3);
        ref_colors[3] = 0xff000000 | b << 16 | g << 8 | r;
    } else {
        let r = r0.wrapping_add(r1).wrapping_add(1) >> 1;
        let g = g0.wrapping_add(g1).wrapping_add(1) >> 1;
        let b = b0.wrapping_add(b1).wrapping_add(1) >> 1;
        ref_colors[2] = 0xff000000 | b << 16 | g << 8 | r;
        ref_colors[3] = 0;
    }

    let mut color_indices = *(compressed_block as *const u32).add(1);
    let mut dst_colors = decompressed_block as *mut u8;
    for _ in 0..4 {
        for j in 0..4 {
            *(dst_colors as *mut u32).add(j) = ref_colors[(color_indices & 0x3) as usize];
            color_indices >>= 2;
        }
        dst_colors = dst_colors.add(destination_pitch as usize);
    }
}

unsafe fn sharp_alpha_block(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
) {
    let mut alpha: *mut u16 = std::ptr::null_mut::<u16>();
    let mut decompressed: *mut u8 = std::ptr::null_mut::<u8>();
    let mut i: i32 = 0;
    let mut j: i32 = 0;
    alpha = compressed_block as *mut u16;
    decompressed = decompressed_block as *mut u8;
    i = 0;
    while i < 4 {
        j = 0;
        while j < 4 {
            *decompressed.offset((j * 4) as isize) =
                ((*alpha.offset(i as isize) as i32 >> (4 * j) & 0xf) * 17) as u8;
            j += 1;
        }
        decompressed = decompressed.offset(destination_pitch as isize);
        i += 1;
    }
}

unsafe fn smooth_alpha_block(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
    pixel_size: i32,
) {
    let mut decompressed: *mut u8 = std::ptr::null_mut::<u8>();
    let mut alpha: [u8; 8] = [0; 8];
    let mut i: i32 = 0;
    let mut j: i32 = 0;
    let mut block: u64 = 0;
    let mut indices: u64 = 0;
    block = *(compressed_block as *mut u64);
    decompressed = decompressed_block as *mut u8;
    alpha[0] = (block & 0xff) as u8;
    alpha[1] = (block >> 8 & 0xff) as u8;
    if alpha[0] as i32 > alpha[1] as i32 {
        alpha[2] = ((6 * alpha[0] as i32 + alpha[1] as i32 + 1) / 7) as u8;
        alpha[3] = ((5 * alpha[0] as i32 + 2 * alpha[1] as i32 + 1) / 7) as u8;
        alpha[4] = ((4 * alpha[0] as i32 + 3 * alpha[1] as i32 + 1) / 7) as u8;
        alpha[5] = ((3 * alpha[0] as i32 + 4 * alpha[1] as i32 + 1) / 7) as u8;
        alpha[6] = ((2 * alpha[0] as i32 + 5 * alpha[1] as i32 + 1) / 7) as u8;
        alpha[7] = ((alpha[0] as i32 + 6 * alpha[1] as i32 + 1) / 7) as u8;
    } else {
        alpha[2] = ((4 * alpha[0] as i32 + alpha[1] as i32 + 1) / 5) as u8;
        alpha[3] = ((3 * alpha[0] as i32 + 2 * alpha[1] as i32 + 1) / 5) as u8;
        alpha[4] = ((2 * alpha[0] as i32 + 3 * alpha[1] as i32 + 1) / 5) as u8;
        alpha[5] = ((alpha[0] as i32 + 4 * alpha[1] as i32 + 1) / 5) as u8;
        alpha[6] = 0;
        alpha[7] = 0xff;
    }
    indices = block >> 16;
    i = 0;
    while i < 4 {
        j = 0;
        while j < 4 {
            *decompressed.offset((j * pixel_size) as isize) = alpha[(indices & 0x7) as usize];
            indices >>= 3;
            j += 1;
        }
        decompressed = decompressed.offset(destination_pitch as isize);
        i += 1;
    }
}

unsafe fn bitstream_read_bits(mut bstream: *mut Bitstream, num_bits: i32) -> i32 {
    let mask: u32 = (((1) << num_bits) - 1) as u32;
    let bits: u32 = ((*bstream).low & mask as u64) as u32;
    (*bstream).low >>= num_bits;
    (*bstream).low |= ((*bstream).high & mask as u64)
        << (std::mem::size_of::<u64>() as u32).wrapping_mul(8).wrapping_sub(num_bits as u32);
    (*bstream).high >>= num_bits;
    bits as i32
}

unsafe fn bitstream_read_bit(bstream: *mut Bitstream) -> i32 { bitstream_read_bits(bstream, 1) }

unsafe fn bitstream_read_bits_r(bstream: *mut Bitstream, mut num_bits: i32) -> i32 {
    let mut bits: i32 = bitstream_read_bits(bstream, num_bits);
    let mut result: i32 = 0;
    loop {
        let fresh0 = num_bits;
        num_bits -= 1;
        if fresh0 == 0 {
            break;
        }
        result <<= 1;
        result |= bits & 1;
        bits >>= 1;
    }
    result
}

pub unsafe fn bcdec_bc1(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
) {
    color_block(compressed_block, decompressed_block, destination_pitch, false);
}

pub unsafe fn bcdec_bc2(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
) {
    color_block(
        (compressed_block as *mut u8).offset(8) as *const u8,
        decompressed_block,
        destination_pitch,
        true,
    );
    sharp_alpha_block(
        compressed_block,
        (decompressed_block as *mut u8).offset(3) as *mut u8,
        destination_pitch,
    );
}

pub unsafe fn bcdec_bc3(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
) {
    color_block(
        (compressed_block as *mut u8).offset(8) as *const u8,
        decompressed_block,
        destination_pitch,
        true,
    );
    smooth_alpha_block(
        compressed_block,
        (decompressed_block as *mut u8).offset(3) as *mut u8,
        destination_pitch,
        4,
    );
}

pub unsafe fn bcdec_bc4(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
) {
    smooth_alpha_block(compressed_block, decompressed_block, destination_pitch, 1);
}

pub unsafe fn bcdec_bc5(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
) {
    smooth_alpha_block(compressed_block, decompressed_block, destination_pitch, 2);
    smooth_alpha_block(
        (compressed_block as *mut u8).add(8) as *const u8,
        (decompressed_block as *mut u8).add(1) as *mut u8,
        destination_pitch,
        2,
    );
}

#[inline]
fn extend_sign(val: i32, bits: i32) -> i32 { val << (32 - bits) >> (32 - bits) }

#[inline]
fn transform_inverse(mut val: i32, a0: i32, bits: i32, is_signed: bool) -> i32 {
    val = (val + a0) & (((1) << bits) - 1);
    if is_signed {
        val = extend_sign(val, bits);
    }
    val
}

#[inline]
fn unquantize(mut val: i32, bits: i32, is_signed: bool) -> i32 {
    let mut unq: i32 = 0;
    let mut s: i32 = 0;
    if !is_signed {
        if bits >= 15 {
            unq = val;
        } else if val == 0 {
            unq = 0;
        } else if val == ((1) << bits) - 1 {
            unq = 0xffff;
        } else {
            unq = ((val << 16) + 0x8000) >> bits;
        }
    } else if bits >= 16 {
        unq = val;
    } else {
        if val < 0 {
            s = 1;
            val = -val;
        }
        if val == 0 {
            unq = 0;
        } else if val >= ((1) << (bits - 1)) - 1 {
            unq = 0x7fff;
        } else {
            unq = ((val << 15) + 0x4000) >> (bits - 1);
        }
        if s != 0 {
            unq = -unq;
        }
    }
    unq
}

unsafe fn interpolate(a: i32, b: i32, weights: *const i32, index: i32) -> i32 {
    (a * (64 - *weights.offset(index as isize)) + b * *weights.offset(index as isize) + 32) >> 6
}

#[inline]
fn finish_unquantize(mut val: i32, is_signed: bool) -> u16 {
    if is_signed {
        val = if val < 0 { -((-val * 31) >> 5) } else { (val * 31) >> 5 };
        let mut s = 0;
        if val < 0 {
            s = 0x8000;
            val = -val;
        }
        (s | val) as u16
    } else {
        ((val * 31) >> 6) as u16
    }
}

unsafe fn half_to_float_quick(half: u16) -> f32 {
    #[derive(Copy, Clone)]
    #[repr(C)]
    pub union FP32 {
        pub u: u32,
        pub f: f32,
    }
    static MAGIC: FP32 = FP32 { u: 113u32 << 23 };
    static SHIFTED_EXP: u32 = 0x7c00u32 << 13;
    let mut o: FP32 = FP32 { u: (half as u32 & 0x7fff) << 13 };
    let exp = SHIFTED_EXP & o.u;
    o.u = o.u.wrapping_add(((127 - 15) << 23) as u32);
    if exp == SHIFTED_EXP {
        o.u = o.u.wrapping_add(((128 - 16) << 23) as u32);
    } else if exp == 0 {
        o.u = o.u.wrapping_add(((1) << 23) as u32);
        o.f -= MAGIC.f;
    }
    o.u |= (half as u32 & 0x8000) << 16;
    o.f
}

pub unsafe fn bcdec_bc6h_half(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
    is_signed: bool,
) {
    static ACTUAL_BITS_COUNT: [[u8; 14]; 4] = [
        [10, 7, 11, 11, 11, 9, 8, 8, 8, 6, 10, 11, 12, 16],
        [5, 6, 5, 4, 4, 5, 6, 5, 5, 6, 10, 9, 8, 4],
        [5, 6, 4, 5, 4, 5, 5, 6, 5, 6, 10, 9, 8, 4],
        [5, 6, 4, 4, 5, 5, 5, 5, 6, 6, 10, 9, 8, 4],
    ];
    static PARTITION_SETS: [[[u8; 4]; 4]; 32] = [
        [[128, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 129]],
        [[128, 0, 0, 1], [0, 0, 0, 1], [0, 0, 0, 1], [0, 0, 0, 129]],
        [[128, 1, 1, 1], [0, 1, 1, 1], [0, 1, 1, 1], [0, 1, 1, 129]],
        [[128, 0, 0, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 1, 1, 129]],
        [[128, 0, 0, 0], [0, 0, 0, 1], [0, 0, 0, 1], [0, 0, 1, 129]],
        [[128, 0, 1, 1], [0, 1, 1, 1], [0, 1, 1, 1], [1, 1, 1, 129]],
        [[128, 0, 0, 1], [0, 0, 1, 1], [0, 1, 1, 1], [1, 1, 1, 129]],
        [[128, 0, 0, 0], [0, 0, 0, 1], [0, 0, 1, 1], [0, 1, 1, 129]],
        [[128, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 1], [0, 0, 1, 129]],
        [[128, 0, 1, 1], [0, 1, 1, 1], [1, 1, 1, 1], [1, 1, 1, 129]],
        [[128, 0, 0, 0], [0, 0, 0, 1], [0, 1, 1, 1], [1, 1, 1, 129]],
        [[128, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 1], [0, 1, 1, 129]],
        [[128, 0, 0, 1], [0, 1, 1, 1], [1, 1, 1, 1], [1, 1, 1, 129]],
        [[128, 0, 0, 0], [0, 0, 0, 0], [1, 1, 1, 1], [1, 1, 1, 129]],
        [[128, 0, 0, 0], [1, 1, 1, 1], [1, 1, 1, 1], [1, 1, 1, 129]],
        [[128, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [1, 1, 1, 129]],
        [[128, 0, 0, 0], [1, 0, 0, 0], [1, 1, 1, 0], [1, 1, 1, 129]],
        [[128, 1, 129, 1], [0, 0, 0, 1], [0, 0, 0, 0], [0, 0, 0, 0]],
        [[128, 0, 0, 0], [0, 0, 0, 0], [129, 0, 0, 0], [1, 1, 1, 0]],
        [[128, 1, 129, 1], [0, 0, 1, 1], [0, 0, 0, 1], [0, 0, 0, 0]],
        [[128, 0, 129, 1], [0, 0, 0, 1], [0, 0, 0, 0], [0, 0, 0, 0]],
        [[128, 0, 0, 0], [1, 0, 0, 0], [129, 1, 0, 0], [1, 1, 1, 0]],
        [[128, 0, 0, 0], [0, 0, 0, 0], [129, 0, 0, 0], [1, 1, 0, 0]],
        [[128, 1, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 0, 129]],
        [[128, 0, 129, 1], [0, 0, 0, 1], [0, 0, 0, 1], [0, 0, 0, 0]],
        [[128, 0, 0, 0], [1, 0, 0, 0], [129, 0, 0, 0], [1, 1, 0, 0]],
        [[128, 1, 129, 0], [0, 1, 1, 0], [0, 1, 1, 0], [0, 1, 1, 0]],
        [[128, 0, 129, 1], [0, 1, 1, 0], [0, 1, 1, 0], [1, 1, 0, 0]],
        [[128, 0, 0, 1], [0, 1, 1, 1], [129, 1, 1, 0], [1, 0, 0, 0]],
        [[128, 0, 0, 0], [1, 1, 1, 1], [129, 1, 1, 1], [0, 0, 0, 0]],
        [[128, 1, 129, 1], [0, 0, 0, 1], [1, 0, 0, 0], [1, 1, 1, 0]],
        [[128, 0, 129, 1], [1, 0, 0, 1], [1, 0, 0, 1], [1, 1, 0, 0]],
    ];
    static A_WEIGHT3: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
    static A_WEIGHT4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];
    let mut bstream: Bitstream = Bitstream { low: 0, high: 0 };
    let mut mode: i32 = 0;
    let mut partition: i32 = 0;
    let mut num_partitions: i32 = 0;
    let mut i: i32 = 0;
    let mut j: i32 = 0;
    let mut partition_set: i32 = 0;
    let mut index_bits: i32 = 0;
    let mut index: i32 = 0;
    let mut ep_i: i32 = 0;
    let mut actual_bits0mode: i32 = 0;
    let mut r: [i32; 4] = [0; 4];
    let mut g: [i32; 4] = [0; 4];
    let mut b: [i32; 4] = [0; 4];
    let mut decompressed: *mut u16 = std::ptr::null_mut::<u16>();
    let mut weights: *const i32 = std::ptr::null::<i32>();
    decompressed = decompressed_block as *mut u16;
    bstream.low = *(compressed_block as *mut u64).offset(0);
    bstream.high = *(compressed_block as *mut u64).offset(1);
    r[3] = 0;
    r[2] = r[3];
    r[1] = r[2];
    r[0] = r[1];
    g[3] = 0;
    g[2] = g[3];
    g[1] = g[2];
    g[0] = g[1];
    b[3] = 0;
    b[2] = b[3];
    b[1] = b[2];
    b[0] = b[1];
    mode = bitstream_read_bits(&mut bstream, 2);
    if mode > 1 {
        mode |= bitstream_read_bits(&mut bstream, 3) << 2;
    }
    partition = 0;
    match mode {
        0 => {
            g[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[3] |= bitstream_read_bit(&mut bstream) << 4;
            r[0] |= bitstream_read_bits(&mut bstream, 10);
            g[0] |= bitstream_read_bits(&mut bstream, 10);
            b[0] |= bitstream_read_bits(&mut bstream, 10);
            r[1] |= bitstream_read_bits(&mut bstream, 5);
            g[3] |= bitstream_read_bit(&mut bstream) << 4;
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            r[3] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 0;
        }
        1 => {
            g[2] |= bitstream_read_bit(&mut bstream) << 5;
            g[3] |= bitstream_read_bit(&mut bstream) << 4;
            g[3] |= bitstream_read_bit(&mut bstream) << 5;
            r[0] |= bitstream_read_bits(&mut bstream, 7);
            b[3] |= bitstream_read_bit(&mut bstream);
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bit(&mut bstream) << 4;
            g[0] |= bitstream_read_bits(&mut bstream, 7);
            b[2] |= bitstream_read_bit(&mut bstream) << 5;
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            g[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[0] |= bitstream_read_bits(&mut bstream, 7);
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            b[3] |= bitstream_read_bit(&mut bstream) << 5;
            b[3] |= bitstream_read_bit(&mut bstream) << 4;
            r[1] |= bitstream_read_bits(&mut bstream, 6);
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 6);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 6);
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 6);
            r[3] |= bitstream_read_bits(&mut bstream, 6);
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 1;
        }
        2 => {
            r[0] |= bitstream_read_bits(&mut bstream, 10);
            g[0] |= bitstream_read_bits(&mut bstream, 10);
            b[0] |= bitstream_read_bits(&mut bstream, 10);
            r[1] |= bitstream_read_bits(&mut bstream, 5);
            r[0] |= bitstream_read_bit(&mut bstream) << 10;
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 4);
            g[0] |= bitstream_read_bit(&mut bstream) << 10;
            b[3] |= bitstream_read_bit(&mut bstream);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 4);
            b[0] |= bitstream_read_bit(&mut bstream) << 10;
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            r[3] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 2;
        }
        6 => {
            r[0] |= bitstream_read_bits(&mut bstream, 10);
            g[0] |= bitstream_read_bits(&mut bstream, 10);
            b[0] |= bitstream_read_bits(&mut bstream, 10);
            r[1] |= bitstream_read_bits(&mut bstream, 4);
            r[0] |= bitstream_read_bit(&mut bstream) << 10;
            g[3] |= bitstream_read_bit(&mut bstream) << 4;
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 5);
            g[0] |= bitstream_read_bit(&mut bstream) << 10;
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 4);
            b[0] |= bitstream_read_bit(&mut bstream) << 10;
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 4);
            b[3] |= bitstream_read_bit(&mut bstream);
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            r[3] |= bitstream_read_bits(&mut bstream, 4);
            g[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 3;
        }
        10 => {
            r[0] |= bitstream_read_bits(&mut bstream, 10);
            g[0] |= bitstream_read_bits(&mut bstream, 10);
            b[0] |= bitstream_read_bits(&mut bstream, 10);
            r[1] |= bitstream_read_bits(&mut bstream, 4);
            r[0] |= bitstream_read_bit(&mut bstream) << 10;
            b[2] |= bitstream_read_bit(&mut bstream) << 4;
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 4);
            g[0] |= bitstream_read_bit(&mut bstream) << 10;
            b[3] |= bitstream_read_bit(&mut bstream);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 5);
            b[0] |= bitstream_read_bit(&mut bstream) << 10;
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 4);
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            r[3] |= bitstream_read_bits(&mut bstream, 4);
            b[3] |= bitstream_read_bit(&mut bstream) << 4;
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 4;
        }
        14 => {
            r[0] |= bitstream_read_bits(&mut bstream, 9);
            b[2] |= bitstream_read_bit(&mut bstream) << 4;
            g[0] |= bitstream_read_bits(&mut bstream, 9);
            g[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[0] |= bitstream_read_bits(&mut bstream, 9);
            b[3] |= bitstream_read_bit(&mut bstream) << 4;
            r[1] |= bitstream_read_bits(&mut bstream, 5);
            g[3] |= bitstream_read_bit(&mut bstream) << 4;
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            r[3] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 5;
        }
        18 => {
            r[0] |= bitstream_read_bits(&mut bstream, 8);
            g[3] |= bitstream_read_bit(&mut bstream) << 4;
            b[2] |= bitstream_read_bit(&mut bstream) << 4;
            g[0] |= bitstream_read_bits(&mut bstream, 8);
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            g[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[0] |= bitstream_read_bits(&mut bstream, 8);
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            b[3] |= bitstream_read_bit(&mut bstream) << 4;
            r[1] |= bitstream_read_bits(&mut bstream, 6);
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 6);
            r[3] |= bitstream_read_bits(&mut bstream, 6);
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 6;
        }
        22 => {
            r[0] |= bitstream_read_bits(&mut bstream, 8);
            b[3] |= bitstream_read_bit(&mut bstream);
            b[2] |= bitstream_read_bit(&mut bstream) << 4;
            g[0] |= bitstream_read_bits(&mut bstream, 8);
            g[2] |= bitstream_read_bit(&mut bstream) << 5;
            g[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[0] |= bitstream_read_bits(&mut bstream, 8);
            g[3] |= bitstream_read_bit(&mut bstream) << 5;
            b[3] |= bitstream_read_bit(&mut bstream) << 4;
            r[1] |= bitstream_read_bits(&mut bstream, 5);
            g[3] |= bitstream_read_bit(&mut bstream) << 4;
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 6);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            r[3] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 7;
        }
        26 => {
            r[0] |= bitstream_read_bits(&mut bstream, 8);
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bit(&mut bstream) << 4;
            g[0] |= bitstream_read_bits(&mut bstream, 8);
            b[2] |= bitstream_read_bit(&mut bstream) << 5;
            g[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[0] |= bitstream_read_bits(&mut bstream, 8);
            b[3] |= bitstream_read_bit(&mut bstream) << 5;
            b[3] |= bitstream_read_bit(&mut bstream) << 4;
            r[1] |= bitstream_read_bits(&mut bstream, 5);
            g[3] |= bitstream_read_bit(&mut bstream) << 4;
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 6);
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            r[3] |= bitstream_read_bits(&mut bstream, 5);
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 8;
        }
        30 => {
            r[0] |= bitstream_read_bits(&mut bstream, 6);
            g[3] |= bitstream_read_bit(&mut bstream) << 4;
            b[3] |= bitstream_read_bit(&mut bstream);
            b[3] |= bitstream_read_bit(&mut bstream) << 1;
            b[2] |= bitstream_read_bit(&mut bstream) << 4;
            g[0] |= bitstream_read_bits(&mut bstream, 6);
            g[2] |= bitstream_read_bit(&mut bstream) << 5;
            b[2] |= bitstream_read_bit(&mut bstream) << 5;
            b[3] |= bitstream_read_bit(&mut bstream) << 2;
            g[2] |= bitstream_read_bit(&mut bstream) << 4;
            b[0] |= bitstream_read_bits(&mut bstream, 6);
            g[3] |= bitstream_read_bit(&mut bstream) << 5;
            b[3] |= bitstream_read_bit(&mut bstream) << 3;
            b[3] |= bitstream_read_bit(&mut bstream) << 5;
            b[3] |= bitstream_read_bit(&mut bstream) << 4;
            r[1] |= bitstream_read_bits(&mut bstream, 6);
            g[2] |= bitstream_read_bits(&mut bstream, 4);
            g[1] |= bitstream_read_bits(&mut bstream, 6);
            g[3] |= bitstream_read_bits(&mut bstream, 4);
            b[1] |= bitstream_read_bits(&mut bstream, 6);
            b[2] |= bitstream_read_bits(&mut bstream, 4);
            r[2] |= bitstream_read_bits(&mut bstream, 6);
            r[3] |= bitstream_read_bits(&mut bstream, 6);
            partition = bitstream_read_bits(&mut bstream, 5);
            mode = 9;
        }
        3 => {
            r[0] |= bitstream_read_bits(&mut bstream, 10);
            g[0] |= bitstream_read_bits(&mut bstream, 10);
            b[0] |= bitstream_read_bits(&mut bstream, 10);
            r[1] |= bitstream_read_bits(&mut bstream, 10);
            g[1] |= bitstream_read_bits(&mut bstream, 10);
            b[1] |= bitstream_read_bits(&mut bstream, 10);
            mode = 10;
        }
        7 => {
            r[0] |= bitstream_read_bits(&mut bstream, 10);
            g[0] |= bitstream_read_bits(&mut bstream, 10);
            b[0] |= bitstream_read_bits(&mut bstream, 10);
            r[1] |= bitstream_read_bits(&mut bstream, 9);
            r[0] |= bitstream_read_bit(&mut bstream) << 10;
            g[1] |= bitstream_read_bits(&mut bstream, 9);
            g[0] |= bitstream_read_bit(&mut bstream) << 10;
            b[1] |= bitstream_read_bits(&mut bstream, 9);
            b[0] |= bitstream_read_bit(&mut bstream) << 10;
            mode = 11;
        }
        11 => {
            r[0] |= bitstream_read_bits(&mut bstream, 10);
            g[0] |= bitstream_read_bits(&mut bstream, 10);
            b[0] |= bitstream_read_bits(&mut bstream, 10);
            r[1] |= bitstream_read_bits(&mut bstream, 8);
            r[0] |= bitstream_read_bits_r(&mut bstream, 2) << 10;
            g[1] |= bitstream_read_bits(&mut bstream, 8);
            g[0] |= bitstream_read_bits_r(&mut bstream, 2) << 10;
            b[1] |= bitstream_read_bits(&mut bstream, 8);
            b[0] |= bitstream_read_bits_r(&mut bstream, 2) << 10;
            mode = 12;
        }
        15 => {
            r[0] |= bitstream_read_bits(&mut bstream, 10);
            g[0] |= bitstream_read_bits(&mut bstream, 10);
            b[0] |= bitstream_read_bits(&mut bstream, 10);
            r[1] |= bitstream_read_bits(&mut bstream, 4);
            r[0] |= bitstream_read_bits_r(&mut bstream, 6) << 10;
            g[1] |= bitstream_read_bits(&mut bstream, 4);
            g[0] |= bitstream_read_bits_r(&mut bstream, 6) << 10;
            b[1] |= bitstream_read_bits(&mut bstream, 4);
            b[0] |= bitstream_read_bits_r(&mut bstream, 6) << 10;
            mode = 13;
        }
        _ => {
            i = 0;
            while i < 4 {
                j = 0;
                while j < 4 {
                    *decompressed.offset((j * 3) as isize) = 0;
                    *decompressed.offset((j * 3 + 1) as isize) = 0;
                    *decompressed.offset((j * 3 + 2) as isize) = 0;
                    j += 1;
                }
                decompressed = decompressed.offset(destination_pitch as isize);
                i += 1;
            }
            return;
        }
    }
    num_partitions = if mode >= 10 { 0 } else { 1 };
    actual_bits0mode = ACTUAL_BITS_COUNT[0][mode as usize] as i32;
    if is_signed {
        r[0] = extend_sign(r[0], actual_bits0mode);
        g[0] = extend_sign(g[0], actual_bits0mode);
        b[0] = extend_sign(b[0], actual_bits0mode);
    }
    if mode != 9 && mode != 10 || is_signed {
        i = 1;
        while i < (num_partitions + 1) * 2 {
            r[i as usize] = extend_sign(r[i as usize], ACTUAL_BITS_COUNT[1][mode as usize] as i32);
            g[i as usize] = extend_sign(g[i as usize], ACTUAL_BITS_COUNT[2][mode as usize] as i32);
            b[i as usize] = extend_sign(b[i as usize], ACTUAL_BITS_COUNT[3][mode as usize] as i32);
            i += 1;
        }
    }
    if mode != 9 && mode != 10 {
        i = 1;
        while i < (num_partitions + 1) * 2 {
            r[i as usize] = transform_inverse(r[i as usize], r[0], actual_bits0mode, is_signed);
            g[i as usize] = transform_inverse(g[i as usize], g[0], actual_bits0mode, is_signed);
            b[i as usize] = transform_inverse(b[i as usize], b[0], actual_bits0mode, is_signed);
            i += 1;
        }
    }
    i = 0;
    while i < (num_partitions + 1) * 2 {
        r[i as usize] = unquantize(r[i as usize], actual_bits0mode, is_signed);
        g[i as usize] = unquantize(g[i as usize], actual_bits0mode, is_signed);
        b[i as usize] = unquantize(b[i as usize], actual_bits0mode, is_signed);
        i += 1;
    }
    weights = if mode >= 10 { A_WEIGHT4.as_ptr() } else { A_WEIGHT3.as_ptr() };
    i = 0;
    while i < 4 {
        j = 0;
        while j < 4 {
            partition_set = if mode >= 10 {
                if i | j != 0 {
                    0
                } else {
                    128
                }
            } else {
                PARTITION_SETS[partition as usize][i as usize][j as usize] as i32
            };
            index_bits = if mode >= 10 { 4 } else { 3 };
            if partition_set & 0x80 != 0 {
                index_bits -= 1;
            }
            partition_set &= 0x1;
            index = bitstream_read_bits(&mut bstream, index_bits);
            ep_i = partition_set * 2;
            *decompressed.offset((j * 3) as isize) = finish_unquantize(
                interpolate(r[ep_i as usize], r[(ep_i + 1) as usize], weights, index),
                is_signed,
            );
            *decompressed.offset((j * 3 + 1) as isize) = finish_unquantize(
                interpolate(g[ep_i as usize], g[(ep_i + 1) as usize], weights, index),
                is_signed,
            );
            *decompressed.offset((j * 3 + 2) as isize) = finish_unquantize(
                interpolate(b[ep_i as usize], b[(ep_i + 1) as usize], weights, index),
                is_signed,
            );
            j += 1;
        }
        decompressed = decompressed.offset(destination_pitch as isize);
        i += 1;
    }
}

pub unsafe fn bcdec_bc6h_float(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
    is_signed: bool,
) {
    let mut block: [u16; 48] = [0; 48];
    let mut decompressed: *mut f32 = std::ptr::null_mut::<f32>();
    let mut b: *const u16 = std::ptr::null::<u16>();
    let mut i: i32 = 0;
    let mut j: i32 = 0;
    bcdec_bc6h_half(compressed_block, block.as_mut_ptr() as *mut u8, 4 * 3, is_signed);
    b = block.as_mut_ptr();
    decompressed = decompressed_block as *mut f32;
    i = 0;
    while i < 4 {
        j = 0;
        while j < 4 {
            let fresh1 = b;
            b = b.offset(1);
            *decompressed.offset((j * 3) as isize) = half_to_float_quick(*fresh1);
            let fresh2 = b;
            b = b.offset(1);
            *decompressed.offset((j * 3 + 1) as isize) = half_to_float_quick(*fresh2);
            let fresh3 = b;
            b = b.offset(1);
            *decompressed.offset((j * 3 + 2) as isize) = half_to_float_quick(*fresh3);
            j += 1;
        }
        decompressed = decompressed.offset(destination_pitch as isize);
        i += 1;
    }
}

unsafe fn swap_values(a: *mut i32, b: *mut i32) {
    std::mem::swap(a.as_mut().unwrap_unchecked(), b.as_mut().unwrap_unchecked());
}

pub unsafe fn bcdec_bc7(
    compressed_block: *const u8,
    decompressed_block: *mut u8,
    destination_pitch: u32,
) {
    static ACTUAL_BITS_COUNT: [[u8; 8]; 2] = [[4, 6, 5, 7, 5, 7, 7, 5], [0, 0, 0, 0, 6, 8, 7, 5]];
    static PARTITION_SETS: [[[[u8; 4]; 4]; 64]; 2] = [
        [
            [[128, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 1, 129]],
            [[128, 0, 0, 1], [0, 0, 0, 1], [0, 0, 0, 1], [0, 0, 0, 129]],
            [[128, 1, 1, 1], [0, 1, 1, 1], [0, 1, 1, 1], [0, 1, 1, 129]],
            [[128, 0, 0, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 1, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 1], [0, 0, 0, 1], [0, 0, 1, 129]],
            [[128, 0, 1, 1], [0, 1, 1, 1], [0, 1, 1, 1], [1, 1, 1, 129]],
            [[128, 0, 0, 1], [0, 0, 1, 1], [0, 1, 1, 1], [1, 1, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 1], [0, 0, 1, 1], [0, 1, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 1], [0, 0, 1, 129]],
            [[128, 0, 1, 1], [0, 1, 1, 1], [1, 1, 1, 1], [1, 1, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 1], [0, 1, 1, 1], [1, 1, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 1], [0, 1, 1, 129]],
            [[128, 0, 0, 1], [0, 1, 1, 1], [1, 1, 1, 1], [1, 1, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [1, 1, 1, 1], [1, 1, 1, 129]],
            [[128, 0, 0, 0], [1, 1, 1, 1], [1, 1, 1, 1], [1, 1, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [1, 1, 1, 129]],
            [[128, 0, 0, 0], [1, 0, 0, 0], [1, 1, 1, 0], [1, 1, 1, 129]],
            [[128, 1, 129, 1], [0, 0, 0, 1], [0, 0, 0, 0], [0, 0, 0, 0]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [129, 0, 0, 0], [1, 1, 1, 0]],
            [[128, 1, 129, 1], [0, 0, 1, 1], [0, 0, 0, 1], [0, 0, 0, 0]],
            [[128, 0, 129, 1], [0, 0, 0, 1], [0, 0, 0, 0], [0, 0, 0, 0]],
            [[128, 0, 0, 0], [1, 0, 0, 0], [129, 1, 0, 0], [1, 1, 1, 0]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [129, 0, 0, 0], [1, 1, 0, 0]],
            [[128, 1, 1, 1], [0, 0, 1, 1], [0, 0, 1, 1], [0, 0, 0, 129]],
            [[128, 0, 129, 1], [0, 0, 0, 1], [0, 0, 0, 1], [0, 0, 0, 0]],
            [[128, 0, 0, 0], [1, 0, 0, 0], [129, 0, 0, 0], [1, 1, 0, 0]],
            [[128, 1, 129, 0], [0, 1, 1, 0], [0, 1, 1, 0], [0, 1, 1, 0]],
            [[128, 0, 129, 1], [0, 1, 1, 0], [0, 1, 1, 0], [1, 1, 0, 0]],
            [[128, 0, 0, 1], [0, 1, 1, 1], [129, 1, 1, 0], [1, 0, 0, 0]],
            [[128, 0, 0, 0], [1, 1, 1, 1], [129, 1, 1, 1], [0, 0, 0, 0]],
            [[128, 1, 129, 1], [0, 0, 0, 1], [1, 0, 0, 0], [1, 1, 1, 0]],
            [[128, 0, 129, 1], [1, 0, 0, 1], [1, 0, 0, 1], [1, 1, 0, 0]],
            [[128, 1, 0, 1], [0, 1, 0, 1], [0, 1, 0, 1], [0, 1, 0, 129]],
            [[128, 0, 0, 0], [1, 1, 1, 1], [0, 0, 0, 0], [1, 1, 1, 129]],
            [[128, 1, 0, 1], [1, 0, 129, 0], [0, 1, 0, 1], [1, 0, 1, 0]],
            [[128, 0, 1, 1], [0, 0, 1, 1], [129, 1, 0, 0], [1, 1, 0, 0]],
            [[128, 0, 129, 1], [1, 1, 0, 0], [0, 0, 1, 1], [1, 1, 0, 0]],
            [[128, 1, 0, 1], [0, 1, 0, 1], [129, 0, 1, 0], [1, 0, 1, 0]],
            [[128, 1, 1, 0], [1, 0, 0, 1], [0, 1, 1, 0], [1, 0, 0, 129]],
            [[128, 1, 0, 1], [1, 0, 1, 0], [1, 0, 1, 0], [0, 1, 0, 129]],
            [[128, 1, 129, 1], [0, 0, 1, 1], [1, 1, 0, 0], [1, 1, 1, 0]],
            [[128, 0, 0, 1], [0, 0, 1, 1], [129, 1, 0, 0], [1, 0, 0, 0]],
            [[128, 0, 129, 1], [0, 0, 1, 0], [0, 1, 0, 0], [1, 1, 0, 0]],
            [[128, 0, 129, 1], [1, 0, 1, 1], [1, 1, 0, 1], [1, 1, 0, 0]],
            [[128, 1, 129, 0], [1, 0, 0, 1], [1, 0, 0, 1], [0, 1, 1, 0]],
            [[128, 0, 1, 1], [1, 1, 0, 0], [1, 1, 0, 0], [0, 0, 1, 129]],
            [[128, 1, 1, 0], [0, 1, 1, 0], [1, 0, 0, 1], [1, 0, 0, 129]],
            [[128, 0, 0, 0], [0, 1, 129, 0], [0, 1, 1, 0], [0, 0, 0, 0]],
            [[128, 1, 0, 0], [1, 1, 129, 0], [0, 1, 0, 0], [0, 0, 0, 0]],
            [[128, 0, 129, 0], [0, 1, 1, 1], [0, 0, 1, 0], [0, 0, 0, 0]],
            [[128, 0, 0, 0], [0, 0, 129, 0], [0, 1, 1, 1], [0, 0, 1, 0]],
            [[128, 0, 0, 0], [0, 1, 0, 0], [129, 1, 1, 0], [0, 1, 0, 0]],
            [[128, 1, 1, 0], [1, 1, 0, 0], [1, 0, 0, 1], [0, 0, 1, 129]],
            [[128, 0, 1, 1], [0, 1, 1, 0], [1, 1, 0, 0], [1, 0, 0, 129]],
            [[128, 1, 129, 0], [0, 0, 1, 1], [1, 0, 0, 1], [1, 1, 0, 0]],
            [[128, 0, 129, 1], [1, 0, 0, 1], [1, 1, 0, 0], [0, 1, 1, 0]],
            [[128, 1, 1, 0], [1, 1, 0, 0], [1, 1, 0, 0], [1, 0, 0, 129]],
            [[128, 1, 1, 0], [0, 0, 1, 1], [0, 0, 1, 1], [1, 0, 0, 129]],
            [[128, 1, 1, 1], [1, 1, 1, 0], [1, 0, 0, 0], [0, 0, 0, 129]],
            [[128, 0, 0, 1], [1, 0, 0, 0], [1, 1, 1, 0], [0, 1, 1, 129]],
            [[128, 0, 0, 0], [1, 1, 1, 1], [0, 0, 1, 1], [0, 0, 1, 129]],
            [[128, 0, 129, 1], [0, 0, 1, 1], [1, 1, 1, 1], [0, 0, 0, 0]],
            [[128, 0, 129, 0], [0, 0, 1, 0], [1, 1, 1, 0], [1, 1, 1, 0]],
            [[128, 1, 0, 0], [0, 1, 0, 0], [0, 1, 1, 1], [0, 1, 1, 129]],
        ],
        [
            [[128, 0, 1, 129], [0, 0, 1, 1], [0, 2, 2, 1], [2, 2, 2, 130]],
            [[128, 0, 0, 129], [0, 0, 1, 1], [130, 2, 1, 1], [2, 2, 2, 1]],
            [[128, 0, 0, 0], [2, 0, 0, 1], [130, 2, 1, 1], [2, 2, 1, 129]],
            [[128, 2, 2, 130], [0, 0, 2, 2], [0, 0, 1, 1], [0, 1, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [129, 1, 2, 2], [1, 1, 2, 130]],
            [[128, 0, 1, 129], [0, 0, 1, 1], [0, 0, 2, 2], [0, 0, 2, 130]],
            [[128, 0, 2, 130], [0, 0, 2, 2], [1, 1, 1, 1], [1, 1, 1, 129]],
            [[128, 0, 1, 1], [0, 0, 1, 1], [130, 2, 1, 1], [2, 2, 1, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [129, 1, 1, 1], [2, 2, 2, 130]],
            [[128, 0, 0, 0], [1, 1, 1, 1], [129, 1, 1, 1], [2, 2, 2, 130]],
            [[128, 0, 0, 0], [1, 1, 129, 1], [2, 2, 2, 2], [2, 2, 2, 130]],
            [[128, 0, 1, 2], [0, 0, 129, 2], [0, 0, 1, 2], [0, 0, 1, 130]],
            [[128, 1, 1, 2], [0, 1, 129, 2], [0, 1, 1, 2], [0, 1, 1, 130]],
            [[128, 1, 2, 2], [0, 129, 2, 2], [0, 1, 2, 2], [0, 1, 2, 130]],
            [[128, 0, 1, 129], [0, 1, 1, 2], [1, 1, 2, 2], [1, 2, 2, 130]],
            [[128, 0, 1, 129], [2, 0, 0, 1], [130, 2, 0, 0], [2, 2, 2, 0]],
            [[128, 0, 0, 129], [0, 0, 1, 1], [0, 1, 1, 2], [1, 1, 2, 130]],
            [[128, 1, 1, 129], [0, 0, 1, 1], [130, 0, 0, 1], [2, 2, 0, 0]],
            [[128, 0, 0, 0], [1, 1, 2, 2], [129, 1, 2, 2], [1, 1, 2, 130]],
            [[128, 0, 2, 130], [0, 0, 2, 2], [0, 0, 2, 2], [1, 1, 1, 129]],
            [[128, 1, 1, 129], [0, 1, 1, 1], [0, 2, 2, 2], [0, 2, 2, 130]],
            [[128, 0, 0, 129], [0, 0, 0, 1], [130, 2, 2, 1], [2, 2, 2, 1]],
            [[128, 0, 0, 0], [0, 0, 129, 1], [0, 1, 2, 2], [0, 1, 2, 130]],
            [[128, 0, 0, 0], [1, 1, 0, 0], [130, 2, 129, 0], [2, 2, 1, 0]],
            [[128, 1, 2, 130], [0, 129, 2, 2], [0, 0, 1, 1], [0, 0, 0, 0]],
            [[128, 0, 1, 2], [0, 0, 1, 2], [129, 1, 2, 2], [2, 2, 2, 130]],
            [[128, 1, 1, 0], [1, 2, 130, 1], [129, 2, 2, 1], [0, 1, 1, 0]],
            [[128, 0, 0, 0], [0, 1, 129, 0], [1, 2, 130, 1], [1, 2, 2, 1]],
            [[128, 0, 2, 2], [1, 1, 0, 2], [129, 1, 0, 2], [0, 0, 2, 130]],
            [[128, 1, 1, 0], [0, 129, 1, 0], [2, 0, 0, 2], [2, 2, 2, 130]],
            [[128, 0, 1, 1], [0, 1, 2, 2], [0, 1, 130, 2], [0, 0, 1, 129]],
            [[128, 0, 0, 0], [2, 0, 0, 0], [130, 2, 1, 1], [2, 2, 2, 129]],
            [[128, 0, 0, 0], [0, 0, 0, 2], [129, 1, 2, 2], [1, 2, 2, 130]],
            [[128, 2, 2, 130], [0, 0, 2, 2], [0, 0, 1, 2], [0, 0, 1, 129]],
            [[128, 0, 1, 129], [0, 0, 1, 2], [0, 0, 2, 2], [0, 2, 2, 130]],
            [[128, 1, 2, 0], [0, 129, 2, 0], [0, 1, 130, 0], [0, 1, 2, 0]],
            [[128, 0, 0, 0], [1, 1, 129, 1], [2, 2, 130, 2], [0, 0, 0, 0]],
            [[128, 1, 2, 0], [1, 2, 0, 1], [130, 0, 129, 2], [0, 1, 2, 0]],
            [[128, 1, 2, 0], [2, 0, 1, 2], [129, 130, 0, 1], [0, 1, 2, 0]],
            [[128, 0, 1, 1], [2, 2, 0, 0], [1, 1, 130, 2], [0, 0, 1, 129]],
            [[128, 0, 1, 1], [1, 1, 130, 2], [2, 2, 0, 0], [0, 0, 1, 129]],
            [[128, 1, 0, 129], [0, 1, 0, 1], [2, 2, 2, 2], [2, 2, 2, 130]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [130, 1, 2, 1], [2, 1, 2, 129]],
            [[128, 0, 2, 2], [1, 129, 2, 2], [0, 0, 2, 2], [1, 1, 2, 130]],
            [[128, 0, 2, 130], [0, 0, 1, 1], [0, 0, 2, 2], [0, 0, 1, 129]],
            [[128, 2, 2, 0], [1, 2, 130, 1], [0, 2, 2, 0], [1, 2, 2, 129]],
            [[128, 1, 0, 1], [2, 2, 130, 2], [2, 2, 2, 2], [0, 1, 0, 129]],
            [[128, 0, 0, 0], [2, 1, 2, 1], [130, 1, 2, 1], [2, 1, 2, 129]],
            [[128, 1, 0, 129], [0, 1, 0, 1], [0, 1, 0, 1], [2, 2, 2, 130]],
            [[128, 2, 2, 130], [0, 1, 1, 1], [0, 2, 2, 2], [0, 1, 1, 129]],
            [[128, 0, 0, 2], [1, 129, 1, 2], [0, 0, 0, 2], [1, 1, 1, 130]],
            [[128, 0, 0, 0], [2, 129, 1, 2], [2, 1, 1, 2], [2, 1, 1, 130]],
            [[128, 2, 2, 2], [0, 129, 1, 1], [0, 1, 1, 1], [0, 2, 2, 130]],
            [[128, 0, 0, 2], [1, 1, 1, 2], [129, 1, 1, 2], [0, 0, 0, 130]],
            [[128, 1, 1, 0], [0, 129, 1, 0], [0, 1, 1, 0], [2, 2, 2, 130]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [2, 1, 129, 2], [2, 1, 1, 130]],
            [[128, 1, 1, 0], [0, 129, 1, 0], [2, 2, 2, 2], [2, 2, 2, 130]],
            [[128, 0, 2, 2], [0, 0, 1, 1], [0, 0, 129, 1], [0, 0, 2, 130]],
            [[128, 0, 2, 2], [1, 1, 2, 2], [129, 1, 2, 2], [0, 0, 2, 130]],
            [[128, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [2, 129, 1, 130]],
            [[128, 0, 0, 130], [0, 0, 0, 1], [0, 0, 0, 2], [0, 0, 0, 129]],
            [[128, 2, 2, 2], [1, 2, 2, 2], [0, 2, 2, 2], [129, 2, 2, 130]],
            [[128, 1, 0, 129], [2, 2, 2, 2], [2, 2, 2, 2], [2, 2, 2, 130]],
            [[128, 1, 1, 129], [2, 0, 1, 1], [130, 2, 0, 1], [2, 2, 2, 0]],
        ],
    ];
    static A_WEIGHT2: [i32; 4] = [0, 21, 43, 64];
    static A_WEIGHT3: [i32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
    static A_WEIGHT4: [i32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];
    static S_MODE_HAS_PBITS: u8 = 0o313;
    let mut bstream: Bitstream = Bitstream { low: 0, high: 0 };
    let mut mode: i32 = 0;
    let mut partition: i32 = 0;
    let mut num_partitions: i32 = 0;
    let mut num_endpoints: i32 = 0;
    let mut i: i32 = 0;
    let mut j: i32 = 0;
    let mut k: i32 = 0;
    let mut rotation: i32 = 0;
    let mut partition_set: i32 = 0;
    let mut index_selection_bit: i32 = 0;
    let mut index_bits: i32 = 0;
    let mut index_bits2: i32 = 0;
    let mut index: i32 = 0;
    let mut index2: i32 = 0;
    let mut endpoints: [[i32; 4]; 6] = [[0; 4]; 6];
    let mut indices: [[u8; 4]; 4] = [[0; 4]; 4];
    let mut r: i32 = 0;
    let mut g: i32 = 0;
    let mut b: i32 = 0;
    let mut a: i32 = 0;
    let mut weights: *const i32 = std::ptr::null::<i32>();
    let mut weights2: *const i32 = std::ptr::null::<i32>();
    let mut decompressed: *mut u8 = std::ptr::null_mut::<u8>();
    decompressed = decompressed_block as *mut u8;
    bstream.low = *(compressed_block as *mut u64).offset(0);
    bstream.high = *(compressed_block as *mut u64).offset(1);
    mode = 0;
    while mode < 8 && 0 == bitstream_read_bit(&mut bstream) {
        mode += 1;
    }
    if mode >= 8 {
        i = 0;
        while i < 4 {
            j = 0;
            while j < 4 {
                *decompressed.offset((j * 4) as isize) = 0;
                *decompressed.offset((j * 4 + 1) as isize) = 0;
                *decompressed.offset((j * 4 + 2) as isize) = 0;
                *decompressed.offset((j * 4 + 3) as isize) = 0;
                j += 1;
            }
            decompressed = decompressed.offset(destination_pitch as isize);
            i += 1;
        }
        return;
    }
    partition = 0;
    num_partitions = 1;
    rotation = 0;
    index_selection_bit = 0;
    if mode == 0 || mode == 1 || mode == 2 || mode == 3 || mode == 7 {
        num_partitions = if mode == 0 || mode == 2 { 3 } else { 2 };
        partition = bitstream_read_bits(&mut bstream, if mode == 0 { 4 } else { 6 });
    }
    num_endpoints = num_partitions * 2;
    if mode == 4 || mode == 5 {
        rotation = bitstream_read_bits(&mut bstream, 2);
        if mode == 4 {
            index_selection_bit = bitstream_read_bit(&mut bstream);
        }
    }
    i = 0;
    while i < 3 {
        j = 0;
        while j < num_endpoints {
            endpoints[j as usize][i as usize] =
                bitstream_read_bits(&mut bstream, ACTUAL_BITS_COUNT[0][mode as usize] as i32);
            j += 1;
        }
        i += 1;
    }
    if ACTUAL_BITS_COUNT[1][mode as usize] as i32 > 0 {
        j = 0;
        while j < num_endpoints {
            endpoints[j as usize][3] =
                bitstream_read_bits(&mut bstream, ACTUAL_BITS_COUNT[1][mode as usize] as i32);
            j += 1;
        }
    }
    if mode == 0 || mode == 1 || mode == 3 || mode == 6 || mode == 7 {
        i = 0;
        while i < num_endpoints {
            j = 0;
            while j < 4 {
                endpoints[i as usize][j as usize] <<= 1;
                j += 1;
            }
            i += 1;
        }
        if mode == 1 {
            i = bitstream_read_bit(&mut bstream);
            j = bitstream_read_bit(&mut bstream);
            k = 0;
            while k < 3 {
                endpoints[0][k as usize] |= i;
                endpoints[1][k as usize] |= i;
                endpoints[2][k as usize] |= j;
                endpoints[3][k as usize] |= j;
                k += 1;
            }
        } else if S_MODE_HAS_PBITS as i32 & (1) << mode != 0 {
            i = 0;
            while i < num_endpoints {
                j = bitstream_read_bit(&mut bstream);
                k = 0;
                while k < 4 {
                    endpoints[i as usize][k as usize] |= j;
                    k += 1;
                }
                i += 1;
            }
        }
    }
    i = 0;
    while i < num_endpoints {
        j = ACTUAL_BITS_COUNT[0][mode as usize] as i32 + (S_MODE_HAS_PBITS as i32 >> mode & 1);
        k = 0;
        while k < 3 {
            endpoints[i as usize][k as usize] <<= 8 - j;
            endpoints[i as usize][k as usize] =
                endpoints[i as usize][k as usize] | endpoints[i as usize][k as usize] >> j;
            k += 1;
        }
        j = ACTUAL_BITS_COUNT[1][mode as usize] as i32 + (S_MODE_HAS_PBITS as i32 >> mode & 1);
        endpoints[i as usize][3] <<= 8 - j;
        endpoints[i as usize][3] = endpoints[i as usize][3] | endpoints[i as usize][3] >> j;
        i += 1;
    }
    if ACTUAL_BITS_COUNT[1][mode as usize] == 0 {
        j = 0;
        while j < num_endpoints {
            endpoints[j as usize][3] = 0xff;
            j += 1;
        }
    }
    index_bits = if mode == 0 || mode == 1 {
        3
    } else if mode == 6 {
        4
    } else {
        2
    };
    index_bits2 = if mode == 4 {
        3
    } else if mode == 5 {
        2
    } else {
        0
    };
    weights = if index_bits == 2 {
        A_WEIGHT2.as_ptr()
    } else if index_bits == 3 {
        A_WEIGHT3.as_ptr()
    } else {
        A_WEIGHT4.as_ptr()
    };
    weights2 = if index_bits2 == 2 { A_WEIGHT2.as_ptr() } else { A_WEIGHT3.as_ptr() };
    i = 0;
    while i < 4 {
        j = 0;
        while j < 4 {
            partition_set = if num_partitions == 1 {
                if i | j != 0 {
                    0
                } else {
                    128
                }
            } else {
                PARTITION_SETS[(num_partitions - 2) as usize][partition as usize][i as usize]
                    [j as usize] as i32
            };
            index_bits = if mode == 0 || mode == 1 {
                3
            } else if mode == 6 {
                4
            } else {
                2
            };
            if partition_set & 0x80 != 0 {
                index_bits -= 1;
            }
            indices[i as usize][j as usize] = bitstream_read_bits(&mut bstream, index_bits) as u8;
            j += 1;
        }
        i += 1;
    }
    i = 0;
    while i < 4 {
        j = 0;
        while j < 4 {
            partition_set = if num_partitions == 1 {
                if i | j != 0 {
                    0
                } else {
                    128
                }
            } else {
                PARTITION_SETS[(num_partitions - 2) as usize][partition as usize][i as usize]
                    [j as usize] as i32
            };
            partition_set &= 0x3;
            index = indices[i as usize][j as usize] as i32;
            if index_bits2 == 0 {
                r = interpolate(
                    endpoints[(partition_set * 2) as usize][0],
                    endpoints[(partition_set * 2 + 1) as usize][0],
                    weights,
                    index,
                );
                g = interpolate(
                    endpoints[(partition_set * 2) as usize][1],
                    endpoints[(partition_set * 2 + 1) as usize][1],
                    weights,
                    index,
                );
                b = interpolate(
                    endpoints[(partition_set * 2) as usize][2],
                    endpoints[(partition_set * 2 + 1) as usize][2],
                    weights,
                    index,
                );
                a = interpolate(
                    endpoints[(partition_set * 2) as usize][3],
                    endpoints[(partition_set * 2 + 1) as usize][3],
                    weights,
                    index,
                );
            } else {
                index2 = bitstream_read_bits(
                    &mut bstream,
                    if i | j != 0 { index_bits2 } else { index_bits2 - 1 },
                );
                if index_selection_bit == 0 {
                    r = interpolate(
                        endpoints[(partition_set * 2) as usize][0],
                        endpoints[(partition_set * 2 + 1) as usize][0],
                        weights,
                        index,
                    );
                    g = interpolate(
                        endpoints[(partition_set * 2) as usize][1],
                        endpoints[(partition_set * 2 + 1) as usize][1],
                        weights,
                        index,
                    );
                    b = interpolate(
                        endpoints[(partition_set * 2) as usize][2],
                        endpoints[(partition_set * 2 + 1) as usize][2],
                        weights,
                        index,
                    );
                    a = interpolate(
                        endpoints[(partition_set * 2) as usize][3],
                        endpoints[(partition_set * 2 + 1) as usize][3],
                        weights2,
                        index2,
                    );
                } else {
                    r = interpolate(
                        endpoints[(partition_set * 2) as usize][0],
                        endpoints[(partition_set * 2 + 1) as usize][0],
                        weights2,
                        index2,
                    );
                    g = interpolate(
                        endpoints[(partition_set * 2) as usize][1],
                        endpoints[(partition_set * 2 + 1) as usize][1],
                        weights2,
                        index2,
                    );
                    b = interpolate(
                        endpoints[(partition_set * 2) as usize][2],
                        endpoints[(partition_set * 2 + 1) as usize][2],
                        weights2,
                        index2,
                    );
                    a = interpolate(
                        endpoints[(partition_set * 2) as usize][3],
                        endpoints[(partition_set * 2 + 1) as usize][3],
                        weights,
                        index,
                    );
                }
            }
            match rotation {
                1 => {
                    swap_values(&mut a, &mut r);
                }
                2 => {
                    swap_values(&mut a, &mut g);
                }
                3 => {
                    swap_values(&mut a, &mut b);
                }
                _ => {}
            }
            *decompressed.offset((j * 4) as isize) = r as u8;
            *decompressed.offset((j * 4 + 1) as isize) = g as u8;
            *decompressed.offset((j * 4 + 2) as isize) = b as u8;
            *decompressed.offset((j * 4 + 3) as isize) = a as u8;
            j += 1;
        }
        decompressed = decompressed.offset(destination_pitch as isize);
        i += 1;
    }
}

/* LICENSE:

This software is available under 2 licenses -- choose whichever you prefer.

------------------------------------------------------------------------------
ALTERNATIVE A - MIT License

Copyright (c) 2022 Sergii Kudlai

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

------------------------------------------------------------------------------
ALTERNATIVE B - The Unlicense

This is free and unencumbered software released into the public domain.

Anyone is free to copy, modify, publish, use, compile, sell, or
distribute this software, either in source code form or as a compiled
binary, for any purpose, commercial or non-commercial, and by any
means.

In jurisdictions that recognize copyright laws, the author or authors
of this software dedicate any and all copyright interest in the
software to the public domain. We make this dedication for the benefit
of the public at large and to the detriment of our heirs and
successors. We intend this dedication to be an overt act of
relinquishment in perpetuity of all present and future rights to this
software under copyright law.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

For more information, please refer to <https://unlicense.org>

*/
