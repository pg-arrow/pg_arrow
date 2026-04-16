// ── pg_filenode.map parsing ─────────────────────────────────────────────

const RELMAPPER_MAGIC: u32 = 0x00592717;
/// Maximum mappings per pg_filenode.map (PostgreSQL 17+: 63, older: 62).
/// The file is always 512 bytes regardless of how many entries are used.
const RELMAPPER_FILESIZE: usize = 512 * 2;

/// A single OID → filenode mapping from `pg_filenode.map`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelMapping {
    /// Relation OID (e.g. pg_class = 1259).
    pub mapoid: u32,
    /// Filenode number on disk.
    pub mapfilenode: u32,
}

/// Parsed contents of a `pg_filenode.map` file.
#[derive(Debug)]
pub struct RelMapFile {
    pub num_mappings: u32,
    pub mappings: Vec<RelMapping>,
    pub crc: u32,
}

/// Parse a `pg_filenode.map` file from a 512-byte buffer.
pub fn parse_relmap(buf: &[u8]) -> Result<RelMapFile, String> {
    let magic = u32::from_ne_bytes(buf[0..4].try_into().unwrap());
    if magic != RELMAPPER_MAGIC {
        return Err(format!(
            "pg_filenode.map: bad magic 0x{:08X}, expected 0x{:08X}",
            magic, RELMAPPER_MAGIC
        ));
    }

    let num_mappings = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
    let max_possible = (RELMAPPER_FILESIZE - 8 /* header */ - 4 /* crc */) / 8;
    if num_mappings as usize > max_possible {
        return Err(format!(
            "pg_filenode.map: num_mappings {} exceeds max {}",
            num_mappings, max_possible
        ));
    }

    let mut last_off = 0;
    let mut mappings = Vec::with_capacity(num_mappings as usize);
    for i in 0..num_mappings as usize {
        let off = 8 + i * 8;
        let mapoid = u32::from_ne_bytes(buf[off..off + 4].try_into().unwrap());
        let mapfilenode = u32::from_ne_bytes(buf[off + 4..off + 8].try_into().unwrap());
        mappings.push(RelMapping { mapoid, mapfilenode });
        last_off = off + 8;
    }

    // CRC is the last 4 bytes of the file
    let crc_off = last_off - 4;
    let crc = u32::from_ne_bytes(buf[crc_off..crc_off + 4].try_into().unwrap());

    mappings.sort_by_key(|m| m.mapoid);

    Ok(RelMapFile { num_mappings, mappings, crc })
}
