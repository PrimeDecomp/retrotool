//! Little-endian payloads and per-byte validity used by the buffer ABIs.

pub(super) fn append_bytes(words: &mut Vec<u32>, bytes: &[u8]) {
    for chunk in bytes.chunks(4) {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        words.push(u32::from_le_bytes(word));
    }
}

pub(super) fn append_validity(words: &mut Vec<u32>, known: &[bool]) {
    for chunk in known.chunks(32) {
        let mut mask = 0;
        for (bit, &valid) in chunk.iter().enumerate() {
            mask |= u32::from(valid) << bit;
        }
        words.push(mask);
    }
}

pub(super) fn read_byte(words: &[u32], byte_offset: usize) -> u8 {
    let word = words[byte_offset / 4];
    let shift = (byte_offset % 4) * 8;
    (word >> shift) as u8
}

pub(super) fn byte_is_known(bitmap: &[u32], byte_offset: usize) -> bool {
    let word = bitmap[byte_offset / 32];
    let mask = 1 << (byte_offset % 32);
    word & mask != 0
}
