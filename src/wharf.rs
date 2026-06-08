use anyhow::{Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::info;

const PATCH_MAGIC: u32 = 0x0FEF5F00;

/// Apply a wharf patch file (`.pwr`) to a destination directory.
///
/// Format:
///   1. Magic (4 bytes LE): 0x0FEF5F00
///   2. PatchHeader (protobuf): field 1 = CompressionSettings { algorithm (1=brotli) }
///   3. Rest of file is brotli-compressed, containing protobuf messages:
///      - TargetContainer (tlc.Container — empty for fresh install)
///      - SourceContainer (tlc.Container — file listing)
///      - Per file: SyncHeader → SyncOp frames (DATA = 1, HEY_YOU_DID_IT = 2049)
pub fn apply_patch(pwr_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(pwr_path)
        .with_context(|| format!("failed to open patch file: {}", pwr_path.display()))?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, file);

    // Read the 4-byte magic.
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).context("failed to read magic")?;
    let magic = u32::from_le_bytes(magic);
    if magic != PATCH_MAGIC {
        anyhow::bail!("not a wharf patch (magic {magic:#010x})");
    }

    // Read PatchHeader (uncompressed protobuf).
    let header_frame = read_framed(&mut reader).context("failed to read patch header frame")?;
    let algorithm = parse_patch_header(&header_frame).context("failed to parse patch header")?;
    if algorithm != 1 {
        anyhow::bail!("unsupported compression algorithm {algorithm} (expected 1 = brotli)");
    }

    // Decompress the rest of the file with brotli.
    let mut decompressed = Vec::new();
    let mut decoder = brotli::Decompressor::new(reader.by_ref(), 64 * 1024);
    decoder
        .read_to_end(&mut decompressed)
        .context("failed to decompress patch payload")?;
    let mut data = &decompressed[..];

    // Read TargetContainer (empty for fresh install).
    let _target = read_framed_bytes(&mut data).context("failed to read target container")?;

    // Read SourceContainer — file listing.
    let source_frame = read_framed_bytes(&mut data).context("failed to read source container")?;
    let files = parse_container(&source_frame).context("failed to parse source container")?;
    info!("Source container: {} entries", files.len());

    // Process each file's sync ops.
    for (file_index, entry) in files.iter().enumerate() {
        match entry {
            ContainerEntry::Dir(path) => {
                let full = dest_dir.join(path);
                std::fs::create_dir_all(&full)
                    .with_context(|| format!("failed to create directory: {}", full.display()))?;
            }
            ContainerEntry::Symlink { from, to } => {
                if to.is_empty() {
                    info!("Skipping symlink with empty target: {from}");
                    continue;
                }
                let link = dest_dir.join(from);
                if let Some(parent) = link.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(to, &link)
                    .with_context(|| format!("failed to create symlink: {from} → {to}"))?;
                #[cfg(not(unix))]
                info!("symlinks not supported, skipping: {from} → {to}");
            }
            ContainerEntry::File { path, mode } => {
                let full = dest_dir.join(path);
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("failed to create parent dir for: {}", full.display()))?;
                }

                // Read SyncHeader for this file.
                let _sh = read_framed_bytes(&mut data)
                    .with_context(|| format!("sync header for file {file_index}: {}", path.display()))?;

                // Read SyncOps until HEY_YOU_DID_IT (2049).
                let mut file_data = Vec::new();
                loop {
                    let op_frame = read_framed_bytes(&mut data)
                        .with_context(|| format!("sync op for file {file_index}: {}", path.display()))?;
                    match parse_sync_op(&op_frame).context("sync op")? {
                        (0, _) => {}           // BLOCK_RANGE (skip for fresh install)
                        (1, payload) => {      // DATA
                            file_data.extend_from_slice(&payload);
                        }
                        (2049, _) => break,    // HEY_YOU_DID_IT
                        (t, _) => info!("unknown sync op type {t}"),
                    }
                }

                std::fs::write(&full, &file_data)
                    .with_context(|| format!("failed to write file: {}", full.display()))?;

                // Apply permissions from container mode, then detect executables.
                let final_mode = if file_data.starts_with(b"\x7fELF") || file_data.starts_with(b"#!") {
                    *mode | 0o111
                } else {
                    *mode
                };
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&full, std::fs::Permissions::from_mode(final_mode))
                        .with_context(|| format!("failed to set permissions on: {}", full.display()))?;
                }
            }
        }
    }

    info!("Extracted {} entries", files.len());
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────

fn read_varint(r: &mut impl Read) -> Result<u64> {
    let mut shift = 0;
    let mut value = 0u64;
    loop {
        let mut buf = [0u8; 1];
        r.read_exact(&mut buf)?;
        let byte = buf[0];
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            anyhow::bail!("varint too long");
        }
    }
}

fn read_framed(r: &mut impl Read) -> Result<Vec<u8>> {
    let len = read_varint(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn read_framed_bytes(data: &mut &[u8]) -> Result<Vec<u8>> {
    let len = read_varint_from_slice(data)? as usize;
    if len > data.len() {
        anyhow::bail!("framed message length {len} > remaining {}", data.len());
    }
    let buf = data[..len].to_vec();
    *data = &data[len..];
    Ok(buf)
}

fn read_varint_from_slice(data: &mut &[u8]) -> Result<u64> {
    let mut shift = 0;
    let mut value = 0u64;
    loop {
        if data.is_empty() {
            anyhow::bail!("unexpected end of data");
        }
        let byte = data[0];
        *data = &data[1..];
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            anyhow::bail!("varint too long");
        }
    }
}

// ── Protobuf field readers (operate on byte slice + mutable position) ──

fn read_key(data: &[u8], pos: &mut usize) -> Result<(u64, u64)> {
    let key = read_varint_at(data, pos)?;
    Ok((key >> 3, key & 7))
}

fn read_varint_at(data: &[u8], pos: &mut usize) -> Result<u64> {
    let mut shift = 0;
    let mut value = 0u64;
    loop {
        if *pos >= data.len() {
            anyhow::bail!("unexpected end of protobuf data");
        }
        let byte = data[*pos];
        *pos += 1;
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            anyhow::bail!("varint too long");
        }
    }
}

fn read_bytes<'a>(data: &'a [u8], pos: &mut usize) -> Result<&'a [u8]> {
    let len = read_varint_at(data, pos)? as usize;
    if *pos + len > data.len() {
        anyhow::bail!("bytes field past end of data");
    }
    let slice = &data[*pos..*pos + len];
    *pos += len;
    Ok(slice)
}

fn read_string(data: &[u8], pos: &mut usize) -> Result<String> {
    let bytes = read_bytes(data, pos)?;
    String::from_utf8(bytes.to_vec()).context("invalid UTF-8")
}

fn skip_field(data: &[u8], pos: &mut usize, wire: u64) -> Result<()> {
    match wire {
        0 => { read_varint_at(data, pos)?; }
        1 => *pos += 8,
        2 => { let len = read_varint_at(data, pos)? as usize; *pos += len; }
        5 => *pos += 4,
        3 | 4 => {}
        _ => anyhow::bail!("unknown wire type {wire}"),
    }
    Ok(())
}

// ── PatchHeader parser ────────────────────────────────────────────────

/// PatchHeader { CompressionSettings { algorithm (varint 1), quality (varint 2) } }
fn parse_patch_header(data: &[u8]) -> Result<u64> {
    let mut pos = 0;
    let mut algo = 0;
    while pos < data.len() {
        let (field, wire) = read_key(data, &mut pos)?;
        match (field, wire) {
            (1, 2) => algo = parse_compression_settings(read_bytes(data, &mut pos)?)?,
            _ => skip_field(data, &mut pos, wire)?,
        }
    }
    Ok(algo)
}

fn parse_compression_settings(data: &[u8]) -> Result<u64> {
    let mut pos = 0;
    let mut algo = 0;
    while pos < data.len() {
        let (field, wire) = read_key(data, &mut pos)?;
        match (field, wire) {
            (1, 0) => algo = read_varint_at(data, &mut pos)?,
            _ => skip_field(data, &mut pos, wire)?,
        }
    }
    Ok(algo)
}

// ── SyncOp parser ─────────────────────────────────────────────────────

/// SyncOp { type (varint 1), data (bytes 5) }
fn parse_sync_op(data: &[u8]) -> Result<(u64, Vec<u8>)> {
    let mut pos = 0;
    let mut op_type = 0;
    let mut payload = Vec::new();
    while pos < data.len() {
        let (field, wire) = read_key(data, &mut pos)?;
        match (field, wire) {
            (1, 0) => op_type = read_varint_at(data, &mut pos)?,
            (5, 2) => payload = read_bytes(data, &mut pos)?.to_vec(),
            _ => skip_field(data, &mut pos, wire)?,
        }
    }
    Ok((op_type, payload))
}

// ── Container parser ──────────────────────────────────────────────────

enum ContainerEntry {
    Dir(String),
    File { path: PathBuf, mode: u32 },
    Symlink { from: String, to: String },
}

/// Parse a tlc.Container protobuf message:
///   field 1 = repeated File  { path (1), mode (2), size (3), offset (4) }
///   field 2 = repeated Dir   { path (1), mode (2) }
///   field 3 = repeated Symlink { path (1), mode (2), dest (3) }
fn parse_container(data: &[u8]) -> Result<Vec<ContainerEntry>> {
    let mut pos = 0;
    let mut entries = Vec::new();
    while pos < data.len() {
        let (field, wire) = read_key(data, &mut pos)?;
        match (field, wire) {
            (1, 2) => { // File
                let ed = read_bytes(data, &mut pos)?;
                let (path, mode) = parse_tlc_file(ed);
                entries.push(ContainerEntry::File { path: PathBuf::from(path), mode: mode as u32 });
            }
            (2, 2) => { // Dir
                let ed = read_bytes(data, &mut pos)?;
                let (path, _mode) = parse_tlc_file(ed);
                entries.push(ContainerEntry::Dir(path));
            }
            (3, 2) => { // Symlink
                let ed = read_bytes(data, &mut pos)?;
                let (path, dest) = parse_tlc_symlink(ed);
                entries.push(ContainerEntry::Symlink { from: path, to: dest });
            }
            _ => skip_field(data, &mut pos, wire)?,
        }
    }
    Ok(entries)
}

/// Parse tlc.File or tlc.Dir: path (string 1), mode (uint32 2)
fn parse_tlc_file(data: &[u8]) -> (String, u64) {
    let mut pos = 0;
    let mut path = String::new();
    let mut mode = 0o644u64;
    while pos < data.len() {
        let Ok((field, wire)) = read_key(data, &mut pos) else { break };
        match (field, wire) {
            (1, 2) => { path = read_string(data, &mut pos).unwrap_or_default(); }
            (2, 0) => { mode = read_varint_at(data, &mut pos).unwrap_or(0o644); }
            _ => { let _ = skip_field(data, &mut pos, wire); }
        }
    }
    (path, mode)
}

/// Parse tlc.Symlink: path (string 1), dest (string 3)
fn parse_tlc_symlink(data: &[u8]) -> (String, String) {
    let mut pos = 0;
    let mut path = String::new();
    let mut dest = String::new();
    while pos < data.len() {
        let Ok((field, wire)) = read_key(data, &mut pos) else { break };
        match (field, wire) {
            (1, 2) => { path = read_string(data, &mut pos).unwrap_or_default(); }
            (3, 2) => { dest = read_string(data, &mut pos).unwrap_or_default(); }
            _ => { let _ = skip_field(data, &mut pos, wire); }
        }
    }
    (path, dest)
}
