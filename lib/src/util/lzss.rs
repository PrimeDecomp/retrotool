/// https://wiki.axiodl.com/w/LZSS_Compression
pub fn decompress<const M: u8>(mut input: &[u8], output: &mut [u8]) -> bool {
    let group_len = 2usize.pow(M as u32 - 1);
    let mut out_cur = 0usize;

    let mut header_byte = 0u8;
    let mut group = 0u8;
    while !input.is_empty() {
        if group == 0 {
            header_byte = input[0];
            input = &input[1..];
            group = 8;
        }

        if header_byte & 0x80 == 0 {
            output[out_cur..group_len + out_cur].copy_from_slice(&input[..group_len]);
            input = &input[group_len..];
            out_cur += group_len;
        } else {
            let count = (input[0] as usize >> 4) + (4 - M as usize);
            let length = (((input[0] as usize & 0xF) << 0x8) | input[1] as usize) << (M - 1);
            input = &input[2..];

            let seek = out_cur - length;
            for n in 0..count * group_len {
                output[out_cur + n] = output[seek + n];
            }
            out_cur += count * group_len;
        }

        header_byte <<= 1;
        group -= 1;
    }

    out_cur == output.len()
}
