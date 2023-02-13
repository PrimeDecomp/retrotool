use std::{fs::File, path::Path};

use anyhow::{Context, Result};
use memmap2::{Mmap, MmapOptions};

/// Opens a memory mapped file.
pub fn map_file<P: AsRef<Path>>(path: P) -> Result<Mmap> {
    let file = File::open(&path)
        .with_context(|| format!("Failed to open file '{}'", path.as_ref().display()))?;
    let map = unsafe { MmapOptions::new().map(&file) }
        .with_context(|| format!("Failed to mmap file: '{}'", path.as_ref().display()))?;
    Ok(map)
}
