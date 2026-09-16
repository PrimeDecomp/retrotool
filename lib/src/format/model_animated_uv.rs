//! MPR HEAD/ANUV data and its CEvalSystem instance.
//!
//! Original readers: 7100231c10, 7100234170, 71002336c0, 7100236df0.
//! Evaluators: 7100233bb4..710023414c. Indices address 32-bit context words;
//! the low five bits of an EvalId select a value or time dispatch table.
//! Checked bounds/recursion and finite-time requirements are host validation.
//! Host sin/cos are not a promise of SDK transcendental bit equivalence.

use anyhow::{bail, ensure, Context, Result};

#[derive(Clone, Debug)]
pub struct EvalSystem {
    /// Original bytes, including the backwards endian map and its length suffix.
    pub serialized_context: Vec<u8>,
    pub handles: Vec<Vec<u8>>,
    pub update_id: u32,
    pub set_time_id: u32,
    pub set_phase_id: u32,
}

#[derive(Clone, Debug)]
pub struct ModelAnimatedUvData {
    pub system: EvalSystem,
    pub eval_ids: Vec<u32>,
    /// First three indices evaluate the three transforms. Fourth remains raw.
    pub transform_sets: Vec<[i32; 4]>,
    /// CRenderMesh order, not material-cache order. 0xff means no provider.
    pub mesh_sets: Vec<u8>,
    /// The constructor passes bit0 to CAnimatedUVMaterialData::EMode.
    pub flags: u8,
}

struct Reader<'a> {
    remaining: &'a [u8],
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(n <= self.remaining.len(), "Truncated animated UV data");
        let (v, tail) = self.remaining.split_at(n);
        self.remaining = tail;
        Ok(v)
    }

    fn word(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.take(4)?.try_into()?)) }

    fn count(&mut self, stride: usize) -> Result<usize> {
        let n = self.word()? as usize;
        ensure!(n <= self.remaining.len() / stride, "Invalid animated UV count");
        Ok(n)
    }
}

impl ModelAnimatedUvData {
    /// The first HEAD data source, when it is ANUV. Later source records are
    /// unframed and require their own readers before they can be searched.
    /// The caller retains all remaining bytes; absence here does not claim that
    /// no later ANUV source exists.
    pub fn first_in_header(data_sources: &[u8]) -> Result<Option<Self>> {
        let mut r = Reader { remaining: data_sources };
        if r.word()? == 0 {
            return Ok(None);
        }
        if r.take(4)? != b"ANUV" {
            return Ok(None);
        }
        Ok(Some(Self::parse(r.remaining)?.0))
    }

    /// Read one ANUV source body; return the untouched following header bytes.
    pub fn parse(bytes: &[u8]) -> Result<(Self, &[u8])> {
        let mut r = Reader { remaining: bytes };
        let words = r.count(4)?;
        let handle_count = r.count(4)?;
        let serialized_context = r.take(words * 4)?.to_vec();
        let mut handles = Vec::with_capacity(handle_count);
        for _ in 0..handle_count {
            let size = r.count(1)?;
            ensure!(size % 4 == 0, "ANUV handle has a partial word");
            handles.push(r.take(size)?.to_vec());
        }
        let system = EvalSystem {
            serialized_context,
            handles,
            update_id: r.word()?,
            set_time_id: r.word()?,
            set_phase_id: r.word()?,
        };
        let n = r.count(4)?;
        let eval_ids = (0..n).map(|_| r.word()).collect::<Result<_>>()?;
        let n = r.count(16)?;
        let mut transform_sets = Vec::with_capacity(n);
        for _ in 0..n {
            transform_sets.push([
                r.word()? as i32,
                r.word()? as i32,
                r.word()? as i32,
                r.word()? as i32,
            ]);
        }
        let n = r.count(1)?;
        let mesh_sets = r.take(n)?.to_vec();
        let flags = r.take(1)?[0];
        Ok((Self { system, eval_ids, transform_sets, mesh_sets, flags }, r.remaining))
    }

    pub fn instance(&self) -> Result<EvalInstance> {
        Ok(EvalInstance {
            data: decode_context(&self.system.serialized_context)?,
            system: self.system.clone(),
        })
    }

    pub fn mesh_snapshot(
        &self,
        instance: &EvalInstance,
        mesh: usize,
    ) -> Result<Option<AnimatedUvSnapshot>> {
        let &set = self.mesh_sets.get(mesh).context("ANUV mesh map index out of bounds")?;
        if set == 255 {
            return Ok(None);
        }
        let indices = self
            .transform_sets
            .get(usize::from(set))
            .context("ANUV transform set out of bounds")?;
        let mut snapshot = AnimatedUvSnapshot::identity(u32::from(self.flags & 1));
        for (index, transform) in indices[..3].iter().zip(&mut snapshot.transforms) {
            let id = *self
                .eval_ids
                .get(usize::try_from(*index)?)
                .context("ANUV EvalId index out of bounds")?;
            instance.evaluate(id, transform, 0)?;
        }
        Ok(Some(snapshot))
    }
}

/// Convert the original context's heterogeneous big-endian elements to native
/// little endian. The map runs backwards in two-bit entries: 0=8, 1=4, 2=2,
/// 3=1 byte. Its count suffix sums 0xff bytes and a final smaller byte.
pub fn decode_context(serialized: &[u8]) -> Result<Vec<u8>> {
    let mut cursor = serialized.len();
    let mut count = 0usize;
    loop {
        cursor = cursor.checked_sub(1).context("Missing eval endian-map count")?;
        let v = usize::from(serialized[cursor]);
        count = count.checked_add(v).context("Eval endian-map count overflow")?;
        if v != 255 {
            break;
        }
    }
    let map_start = cursor.checked_sub(count.div_ceil(4)).context("Truncated eval endian map")?;
    let mut native = serialized.to_vec();
    let mut offset = 0;
    for i in 0..count {
        let kind = (serialized[cursor - 1 - i / 4] >> ((i & 3) * 2)) & 3;
        let size = [8, 4, 2, 1][usize::from(kind)];
        ensure!(offset + size <= map_start, "Eval endian map overlaps data");
        native[offset..offset + size].reverse();
        offset += size;
    }
    Ok(native)
}

#[derive(Clone, Debug)]
pub struct AnimatedUvSnapshot {
    pub mode: u32,
    /// Native two-row/four-column matrix bytes, in provider order.
    pub transforms: [[u8; 32]; 3],
}

impl AnimatedUvSnapshot {
    /// Callbacks 7100169a84/7100169b40. Flag bit0 expands option1 to all
    /// three matrices. Option2 ignores flags. Option3 has no bind callback.
    pub fn binding_writes(
        &self,
        option: u32,
        flags: u32,
    ) -> Result<Vec<super::shader_nvn_uniforms::NvnUniformValue<'_>>> {
        use super::shader_nvn_uniforms::NvnUniformValue;
        Ok(match option {
            0 | 3 => vec![],
            1 => self.transforms[..if flags & 1 == 0 { 1 } else { 3 }]
                .iter()
                .enumerate()
                .map(|(i, bytes)| NvnUniformValue::Raw { byte_offset: (i * 32) as u32, bytes })
                .collect(),
            2 => vec![NvnUniformValue::Float(f32::from_le_bytes(
                self.transforms[0][12..16].try_into()?,
            ))],
            _ => bail!("Unknown animated UV binding option"),
        })
    }

    pub fn identity(mode: u32) -> Self {
        let mut matrix = [0; 32];
        matrix[..4].copy_from_slice(&1f32.to_le_bytes());
        matrix[20..24].copy_from_slice(&1f32.to_le_bytes());
        Self { mode, transforms: [matrix; 3] }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= 0x74, "Truncated animated UV material snapshot");
        Ok(Self {
            mode: u32::from_le_bytes(bytes[0x10..0x14].try_into()?),
            transforms: std::array::from_fn(|i| {
                bytes[0x14 + i * 32..0x34 + i * 32].try_into().unwrap()
            }),
        })
    }
}

#[derive(Clone, Debug)]
pub struct EvalInstance {
    pub data: Vec<u8>,
    system: EvalSystem,
}

impl EvalInstance {
    fn bytes(&self, word: u32, size: usize) -> Result<&[u8]> {
        let offset = usize::try_from(word)?.checked_mul(4).context("Eval offset overflow")?;
        self.data
            .get(offset..offset.checked_add(size).context("Eval size overflow")?)
            .context("Eval context index out of bounds")
    }

    fn word(&self, word: u32) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(word, 4)?.try_into()?))
    }

    fn float(&self, word: u32) -> Result<f32> { Ok(f32::from_bits(self.word(word)?)) }

    fn double(&self, word: u32) -> Result<f64> {
        Ok(f64::from_le_bytes(self.bytes(word, 8)?.try_into()?))
    }

    fn write(&mut self, word: u32, bytes: &[u8]) -> Result<()> {
        self.bytes(word, bytes.len())?;
        let at = word as usize * 4;
        self.data[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    pub fn update(&mut self, seconds: f64) -> Result<()> {
        self.time(self.system.update_id, seconds, 0)
    }

    pub fn set_time(&mut self, seconds: f64) -> Result<()> {
        self.time(self.system.set_time_id, seconds, 0)
    }

    pub fn set_phase(&mut self, phase: f64) -> Result<()> {
        self.time(self.system.set_phase_id, phase, 0)
    }

    fn time(&mut self, id: u32, value: f64, depth: usize) -> Result<()> {
        ensure!(depth < 64 && value.is_finite(), "Invalid or cyclic eval time request");
        let at = (id >> 5) & 0xfff;
        match id & 31 {
            0 => {
                let id = self.word(at)?;
                self.time(id, quantize(value * 61440.)? / 1024., depth + 1)?;
            }
            1 => {
                let count = self.word(at)? as i32;
                for i in 0..count.max(0) as u32 {
                    let id = self.word(at + 1 + i)?;
                    self.time(id, value, depth + 1)?;
                }
            }
            kind @ (2 | 4 | 5) => {
                let duration = self.double(at + 4)?;
                let t = if kind == 5 {
                    quantize(value.clamp(0., 1.) * duration * 65536.)? / 65536.
                } else {
                    let t = quantize(65536. * self.double(at + 2)? * value)? / 65536.;
                    if kind == 2 {
                        self.double(at)? + t
                    } else {
                        t
                    }
                };
                self.write(at, &t.to_le_bytes())?;
                let current = if kind == 5 {
                    t
                } else {
                    let cycles = trunc_i32(self.double(at + 6)? * t)?;
                    self.write(at + 9, &cycles.to_le_bytes())?;
                    t - duration * f64::from(cycles)
                };
                self.write(at + 8, &(current as f32).to_le_bytes())?;
            }
            kind @ (3 | 6) => {
                let destination = self.word(at)?;
                let time = self.word(at + 1)?;
                let sample = self.float(time + 8)?;
                let handle = (self.word(at + 3)? & 255) as usize;
                let bytes =
                    self.system.handles.get(handle).context("Eval sample handle out of bounds")?;
                let fetch = |i: i32| -> Result<[u8; 4]> {
                    let offset =
                        usize::try_from(i)?.checked_mul(4).context("Eval sample index overflow")?;
                    Ok(bytes
                        .get(offset..offset + 4)
                        .context("Eval sample index out of bounds")?
                        .try_into()?)
                };
                let value = if kind == 6 {
                    fetch(trunc_i32(f64::from(sample + 0.5))?)?
                } else {
                    let index = trunc_i32(f64::from(sample))?;
                    let fraction = sample - index as f32;
                    let last = self.double(time + 4)? as f32;
                    let next =
                        if (index as f32 + 1.) - last < 0. { index as f32 + 1. } else { last };
                    let a = f32::from_le_bytes(fetch(index)?);
                    let b = f32::from_le_bytes(fetch(trunc_i32(f64::from(next))?)?);
                    ((1. - fraction) * a + fraction * b).to_le_bytes()
                };
                self.write(destination, &value)?;
            }
            _ => bail!("Unknown eval time operation {}", id & 31),
        }
        Ok(())
    }

    fn evaluate(&self, id: u32, output: &mut [u8], depth: usize) -> Result<()> {
        ensure!(depth < 64, "Cyclic eval value request");
        let at = (id >> 5) & 0xfff;
        match id & 31 {
            0 => {
                *output.first_mut().context("Empty eval bit output")? =
                    ((self.word(at)? >> ((id >> 17) & 31)) & 1) as u8;
            }
            1 => {
                let size = (id >> 17) as usize * 4;
                ensure!(size <= output.len(), "Eval value exceeds destination");
                output[..size].copy_from_slice(self.bytes(at, size)?);
            }
            2 => {
                let mut values = [0f32; 5];
                for (i, value) in values.iter_mut().enumerate() {
                    let id = self.word(at + i as u32)?;
                    // These five destinations are single, fully assigned floats.
                    ensure!(id & 31 == 1 && id >> 17 == 1, "ANUV scalar expression needs one word");
                    *value = self.float((id >> 5) & 0xfff)?;
                }
                let [u, v, su, sv, angle] = values;
                let sin = angle.sin();
                let cos = angle.cos();
                let matrix = [
                    cos * su,
                    -sin * sv,
                    0.,
                    (u - 0.5) * cos - (v - 0.5) * sin + 0.5,
                    sin * su,
                    cos * sv,
                    0.,
                    (u - 0.5) * sin + (v - 0.5) * cos + 0.5,
                ];
                ensure!(output.len() >= 32, "Truncated ANUV matrix destination");
                for (v, out) in matrix.iter().zip(output.chunks_exact_mut(4)) {
                    out.copy_from_slice(&v.to_le_bytes());
                }
            }
            kind @ (3 | 4) => {
                ensure!(output.len() >= 32, "Truncated ANUV matrix destination");
                output[..32].copy_from_slice(&AnimatedUvSnapshot::identity(0).transforms[0]);
                if kind == 4 {
                    self.evaluate(self.word(at)?, &mut output[12..16], depth + 1)?;
                }
            }
            _ => bail!("Unknown eval value operation {}", id & 31),
        }
        Ok(())
    }
}

fn quantize(value: f64) -> Result<f64> {
    let value = value + 0.5;
    ensure!(
        value.is_finite() && value >= i64::MIN as f64 && value < -(i64::MIN as f64),
        "Eval time conversion out of range"
    );
    Ok((value as i64) as f64)
}
fn trunc_i32(value: f64) -> Result<i32> {
    ensure!(
        value.is_finite() && value >= f64::from(i32::MIN) && value < f64::from(i32::MAX) + 1.,
        "Eval index conversion out of range"
    );
    Ok(value as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heterogeneous_context_uses_backwards_endian_map() -> Result<()> {
        // Original SerializeInData7100236df0 output for this mixed-size input.
        let input = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0xe4, 4];
        assert_eq!(decode_context(&input)?, [
            8, 7, 6, 5, 4, 3, 2, 1, 12, 11, 10, 9, 14, 13, 15, 0xe4, 4
        ]);
        assert_eq!(decode_context(&[7, 8, 9, 0])?, [7, 8, 9, 0]);
        for bad in [&[][..], &[0xff], &[0, 1], &[0, 0xff]] {
            assert!(decode_context(bad).is_err());
        }
        Ok(())
    }

    #[test]
    fn partial_value_writes_preserve_matrix_and_cycles_reject() -> Result<()> {
        let system = EvalSystem {
            serialized_context: vec![],
            handles: vec![],
            update_id: 0,
            set_time_id: 0,
            set_phase_id: 0,
        };
        let mut instance = EvalInstance { system, data: vec![0; 12] };
        // U-translation expression points at a one-word getter of context word2.
        instance.data[..4].copy_from_slice(&((1u32 << 17) | (2 << 5) | 1).to_le_bytes());
        instance.data[8..12].copy_from_slice(&0.375f32.to_le_bytes());
        let mut matrix = [0x5a; 32];
        instance.evaluate(4, &mut matrix, 0)?;
        assert_eq!(&matrix[12..16], &0.375f32.to_le_bytes());
        matrix[12..16].fill(0);
        assert_eq!(matrix, AnimatedUvSnapshot::identity(0).transforms[0]);
        instance.data[..4].fill(0); // Time operation0 references itself.
        assert!(instance.set_time(0.).is_err());
        assert!(instance.set_time(f64::NAN).is_err());
        assert!(instance.evaluate(31, &mut matrix, 0).is_err());
        Ok(())
    }
}
