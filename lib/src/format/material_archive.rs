//! MPR's version 22 shader archive, embedded in cooked version 168 MTRL resources.
//!
//! The layout and TOC offset base follow NMaterialArchive::ImportMaterialArchive
//! (0x710017f140), ImportMaterialArchiveWithTOC (0x710017fb20), and the stream
//! constructors at 0x710017d544..0x710017ed74. Backend program data and reflection
//! metadata remain opaque: they are not portable shader source.

use anyhow::{ensure, Context, Result};
use uuid::Uuid;
use zerocopy::{ByteOrder, LittleEndian};

use crate::format::{chunk::ChunkDescriptor, mtrl::K_FORM_MTRL, rfrm::FormDescriptor, FourCC};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShaderIdentifier {
    /// ASCII technique code: x/z/g/i/f/s/w/m. No alignment before the permutation.
    pub technique: u8,
    pub permutation: [u64; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferTocEntry {
    /// Byte offset from immediately after the backend TOC, at shader_source_count.
    pub offset: u32,
    /// Entire serialized record size, including any length prefixes.
    pub size: u32,
}

#[derive(Debug)]
pub struct ShaderTocEntry {
    pub identifier: ShaderIdentifier,
    pub buffer: BufferTocEntry,
}

#[derive(Debug)]
pub struct MaterialTocEntry<'a> {
    pub id: Uuid,
    pub hash: u32,
    pub name: &'a str,
    pub unknown: u16,
    pub shaders: Vec<ShaderTocEntry>,
    /// Points to the length-prefixed material data blob, not the material header.
    pub data: BufferTocEntry,
}

#[derive(Debug)]
pub struct ArchiveToc<'a> {
    pub sources: Vec<BufferTocEntry>,
    pub materials: Vec<MaterialTocEntry<'a>>,
}

#[derive(Debug)]
pub struct ShaderSource<'a> {
    pub state: u8,
    pub source: &'a [u8],
    pub binary: &'a [u8],
}

#[derive(Debug)]
pub struct ShaderImport<'a> {
    pub identifier: ShaderIdentifier,
    pub hash: u64,
    /// Indices into this backend's source array; -1 means absent.
    pub vertex_program: i32,
    pub pixel_program: i32,
    pub geometry_program: i32,
    pub compute_program: i32,
    /// A fifth serialized signed word. Its meaning is not yet established.
    pub unknown: i32,
    pub defines: Vec<&'a str>,
    pub defines_unknown: u32,
    pub metadata: &'a [u8],
}

#[derive(Debug)]
pub struct MaterialImport<'a> {
    pub id: Uuid,
    pub hash: u32,
    pub name: &'a str,
    pub unknown: u16,
    pub shaders: Vec<ShaderImport<'a>>,
    pub data: &'a [u8],
}

#[derive(Debug)]
pub struct MaterialBackend<'a> {
    pub id: FourCC,
    pub flags: u32,
    pub toc: Option<ArchiveToc<'a>>,
    pub sources: Vec<ShaderSource<'a>>,
    pub materials: Vec<MaterialImport<'a>>,
}

#[derive(Debug)]
pub struct MaterialArchive<'a> {
    pub backends: Vec<MaterialBackend<'a>>,
}

impl<'a> MaterialArchive<'a> {
    /// Read a standalone RFRM/MTRL version 22, returning any trailing bytes.
    pub fn slice(data: &'a [u8]) -> Result<(Self, &'a [u8])> {
        let (desc, mut data, remain) = FormDescriptor::<LittleEndian>::slice(data)?;
        ensure!(desc.id == K_FORM_MTRL, "Expected an MTRL archive");
        ensure!(desc.reader_version.get() == 22, "Unsupported shader archive reader version");
        ensure!(desc.writer_version.get() == 22, "Unsupported shader archive writer version");
        let mut backends = Vec::new();
        while !data.is_empty() {
            let (chunk, payload, next) = ChunkDescriptor::<LittleEndian>::slice(data)?;
            ensure!(chunk.unk.get() == 1, "Unsupported {} backend version", chunk.id);
            backends.push(
                MaterialBackend::read(chunk.id, payload)
                    .with_context(|| format!("Reading {} shader backend", chunk.id))?,
            );
            data = next;
        }
        Ok((Self { backends }, remain))
    }
}

impl<'a> MaterialBackend<'a> {
    fn read(id: FourCC, data: &'a [u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let flags = r.u32()?;
        let toc = if flags & 1 != 0 {
            let mut t = Reader::new(r.blob()?);
            let mut sources = Vec::new();
            for _ in 0..t.count(8)? {
                sources.push(t.buffer()?);
            }
            let mut materials = Vec::new();
            for _ in 0..t.count(38)? {
                let (id, hash, name, unknown) = t.material_header()?;
                let mut shaders = Vec::new();
                for _ in 0..t.count(25)? {
                    shaders
                        .push(ShaderTocEntry { identifier: t.identifier()?, buffer: t.buffer()? });
                }
                materials.push(MaterialTocEntry {
                    id,
                    hash,
                    name,
                    unknown,
                    shaders,
                    data: t.buffer()?,
                });
            }
            t.finish()?;
            Some(ArchiveToc { sources, materials })
        } else {
            None
        };
        // All TOC ranges use this origin, including material/shader ranges.
        let base = r.pos;
        let source_count = r.count(9)?;
        if let Some(toc) = &toc {
            ensure!(toc.sources.len() == source_count, "Source TOC count mismatch");
        }
        let mut sources = Vec::new();
        for i in 0..source_count {
            let start = r.pos;
            sources.push(ShaderSource { state: r.u8()?, source: r.blob()?, binary: r.blob()? });
            if let Some(toc) = &toc {
                toc.sources[i].check(start - base, r.pos - start)?;
            }
        }
        let material_count = r.count(34)?;
        if let Some(toc) = &toc {
            ensure!(toc.materials.len() == material_count, "Material TOC count mismatch");
        }
        let mut materials = Vec::new();
        for mi in 0..material_count {
            let (id, hash, name, unknown) = r.material_header()?;
            let mtoc = toc.as_ref().map(|t| &t.materials[mi]);
            let shader_count = r.count(57)?;
            if let Some(t) = mtoc {
                ensure!(
                    (id, hash, name, unknown) == (t.id, t.hash, t.name, t.unknown),
                    "Material TOC header mismatch"
                );
                ensure!(shader_count == t.shaders.len(), "Shader TOC count mismatch");
            }
            let mut shaders = Vec::new();
            for si in 0..shader_count {
                let start = r.pos;
                let identifier = r.identifier()?;
                let hash = r.u64()?;
                let vertex_program = r.i32()?;
                let pixel_program = r.i32()?;
                let geometry_program = r.i32()?;
                let compute_program = r.i32()?;
                let unknown = r.i32()?;
                for idx in [vertex_program, pixel_program, geometry_program, compute_program] {
                    ensure!(
                        idx == -1 || (idx >= 0 && (idx as usize) < sources.len()),
                        "Shader source index {idx} out of range"
                    );
                }
                let mut defines = Vec::new();
                for _ in 0..r.count(4)? {
                    defines.push(r.string()?);
                }
                let defines_unknown = r.u32()?;
                let metadata = r.blob()?;
                if let Some(t) = mtoc {
                    ensure!(
                        t.shaders[si].identifier == identifier,
                        "Shader TOC identifier mismatch"
                    );
                    t.shaders[si].buffer.check(start - base, r.pos - start)?;
                }
                shaders.push(ShaderImport {
                    identifier,
                    hash,
                    vertex_program,
                    pixel_program,
                    geometry_program,
                    compute_program,
                    unknown,
                    defines,
                    defines_unknown,
                    metadata,
                });
            }
            let start = r.pos;
            let data = r.blob()?;
            if let Some(t) = mtoc {
                t.data.check(start - base, r.pos - start)?;
            }
            materials.push(MaterialImport { id, hash, name, unknown, shaders, data });
        }
        r.finish()?;
        Ok(Self { id, flags, toc, sources, materials })
    }
}

impl BufferTocEntry {
    fn check(self, offset: usize, size: usize) -> Result<()> {
        ensure!(
            self.offset as usize == offset && self.size as usize == size,
            "TOC range mismatch: stored {}+{}, record {offset}+{size}",
            self.offset,
            self.size
        );
        Ok(())
    }
}

pub(super) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self { Self { data, pos: 0 } }

    pub(super) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(
            n <= self.data.len() - self.pos,
            "Truncated archive record at {:#x}: need {n} bytes",
            self.pos
        );
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub(super) fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }

    pub(super) fn u16(&mut self) -> Result<u16> { Ok(LittleEndian::read_u16(self.take(2)?)) }

    pub(super) fn u32(&mut self) -> Result<u32> { Ok(LittleEndian::read_u32(self.take(4)?)) }

    pub(super) fn i32(&mut self) -> Result<i32> { Ok(self.u32()? as i32) }

    pub(super) fn u64(&mut self) -> Result<u64> { Ok(LittleEndian::read_u64(self.take(8)?)) }

    pub(super) fn blob(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    pub(super) fn string(&mut self) -> Result<&'a str> { Ok(std::str::from_utf8(self.blob()?)?) }

    pub(super) fn count(&mut self, min_size: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        ensure!(
            n <= (self.data.len() - self.pos) / min_size,
            "Archive count exceeds remaining data"
        );
        Ok(n)
    }

    pub(super) fn identifier(&mut self) -> Result<ShaderIdentifier> {
        Ok(ShaderIdentifier { technique: self.u8()?, permutation: [self.u64()?, self.u64()?] })
    }

    pub(super) fn buffer(&mut self) -> Result<BufferTocEntry> {
        Ok(BufferTocEntry { offset: self.u32()?, size: self.u32()? })
    }

    pub(super) fn material_header(&mut self) -> Result<(Uuid, u32, &'a str, u16)> {
        Ok((
            Uuid::from_bytes_le(self.take(16)?.try_into()?),
            self.u32()?,
            self.string()?,
            self.u16()?,
        ))
    }

    pub(super) fn finish(&self) -> Result<()> {
        ensure!(self.pos == self.data.len(), "Unexpected trailing archive data at {:#x}", self.pos);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(out: &mut Vec<u8>, value: u32) { out.extend_from_slice(&value.to_le_bytes()); }
    fn blob(out: &mut Vec<u8>, data: &[u8]) {
        word(out, data.len() as u32);
        out.extend_from_slice(data);
    }
    fn identifier(out: &mut Vec<u8>) {
        out.push(b'g');
        out.extend_from_slice(&0x0123456789abcdefu64.to_le_bytes());
        out.extend_from_slice(&0xfedcba9876543210u64.to_le_bytes());
    }
    fn material_header(out: &mut Vec<u8>) {
        out.extend_from_slice(&[0x12; 16]);
        word(out, 0x87654321);
        blob(out, b"test!");
        out.extend_from_slice(&1u16.to_le_bytes());
    }
    // Deliberately unaligned fields, nonempty source/defines/metadata, and two
    // independent permutations. TOC ranges below are hand-calculated wire offsets.
    fn fixture(with_toc: bool) -> Vec<u8> {
        let mut payload = Vec::new();
        word(&mut payload, if with_toc { 1 } else { 0 });
        if with_toc {
            let mut toc = Vec::new();
            word(&mut toc, 1);
            word(&mut toc, 4);
            word(&mut toc, 14);
            word(&mut toc, 1);
            material_header(&mut toc);
            word(&mut toc, 1);
            identifier(&mut toc);
            word(&mut toc, 57);
            word(&mut toc, 68);
            word(&mut toc, 125);
            word(&mut toc, 7);
            assert_eq!(toc.len(), 84);
            blob(&mut payload, &toc);
        }
        word(&mut payload, 1);
        payload.push(1);
        blob(&mut payload, b"abc");
        blob(&mut payload, &[0xfe, 0xed]);
        word(&mut payload, 1);
        material_header(&mut payload);
        word(&mut payload, 1);
        identifier(&mut payload);
        payload.extend_from_slice(&0x1122334455667788u64.to_le_bytes());
        for index in [0, 0, u32::MAX, u32::MAX, 7] {
            word(&mut payload, index);
        }
        word(&mut payload, 1);
        blob(&mut payload, b"TEST");
        word(&mut payload, 42);
        blob(&mut payload, &[0x10, 0x20, 0x30]);
        blob(&mut payload, &[0xaa, 0xbb, 0xcc]);
        let mut chunk = b"SNVN".to_vec();
        chunk.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        word(&mut chunk, 1);
        chunk.extend_from_slice(&0u64.to_le_bytes());
        chunk.extend_from_slice(&payload);
        let mut form = b"RFRM".to_vec();
        form.extend_from_slice(&(chunk.len() as u64).to_le_bytes());
        form.extend_from_slice(&0u64.to_le_bytes());
        form.extend_from_slice(b"MTRL");
        word(&mut form, 22);
        word(&mut form, 22);
        form.extend_from_slice(&chunk);
        form
    }

    #[test]
    fn reads_unaligned_archive_with_and_without_toc() {
        for with_toc in [false, true] {
            let data = fixture(with_toc);
            let (archive, remain) = MaterialArchive::slice(&data).unwrap();
            assert!(remain.is_empty());
            let b = &archive.backends[0];
            assert_eq!(b.toc.is_some(), with_toc);
            assert_eq!(b.sources[0].source, b"abc");
            assert_eq!(b.sources[0].binary, [0xfe, 0xed]);
            let m = &b.materials[0];
            assert_eq!(m.name, "test!");
            assert_eq!(m.hash, 0x87654321);
            assert_eq!(m.data, [0xaa, 0xbb, 0xcc]);
            let s = &m.shaders[0];
            assert_eq!(s.identifier.permutation, [0x0123456789abcdef, 0xfedcba9876543210]);
            assert_eq!(s.identifier.technique, b'g');
            assert_eq!(s.geometry_program, -1);
            assert_eq!(s.unknown, 7);
            assert_eq!(s.defines, ["TEST"]);
            assert_eq!(s.defines_unknown, 42);
            assert_eq!(s.metadata, [0x10, 0x20, 0x30]);
        }
    }

    #[test]
    fn rejects_incorrect_toc_origin_and_shader_indices() {
        let mut data = fixture(true);
        // First source TOC offset is 4 relative to shader_source_count, not to
        // the chunk header or the first shader source.
        data[68..72].copy_from_slice(&0u32.to_le_bytes());
        assert!(format!("{:#}", MaterialArchive::slice(&data).unwrap_err())
            .contains("TOC range mismatch"));
        let mut data = fixture(true);
        // Shader starts at 205; identifier + hash occupy 25 bytes.
        data[230..234].copy_from_slice(&1u32.to_le_bytes());
        assert!(MaterialArchive::slice(&data).is_err());
    }

    #[test]
    fn rejects_truncation_oversized_counts_and_lengths_without_panicking() {
        let data = fixture(true);
        for n in 0..data.len() {
            assert!(MaterialArchive::slice(&data[..n]).is_err());
        }
        for (offset, size) in [(4, 8), (36, 8), (48, 8), (148, 4), (153, 4)] {
            let mut broken = data.clone();
            broken[offset..offset + size].fill(0xff);
            assert!(MaterialArchive::slice(&broken).is_err());
        }
    }
}
