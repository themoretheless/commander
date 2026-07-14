//! Fixed-block and FastCDC delta copy into an isolated staging file.

use crate::transfer::TransferState;
use crate::transfer_tuning::{FastPath, TuningSnapshot};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const FIXED_BLOCK_SIZE: usize = 1024 * 1024;
const CDC_MIN_CHUNK: usize = 64 * 1024;
const CDC_AVG_CHUNK: usize = 256 * 1024;
const CDC_MAX_CHUNK: usize = 1024 * 1024;
const MAX_CDC_INDEX_ENTRIES: usize = 1_000_000;

pub const DELTA_MIN_BYTES: u64 = 64 * 1024 * 1024;
pub const CDC_MIN_BYTES: u64 = 512 * 1024 * 1024;
pub const DELTA_MIN_P95_MS: f64 = 15.0;
pub const CDC_MIN_P95_MS: f64 = 40.0;
pub const CDC_MIN_SAMPLES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeltaMode {
    Fixed,
    ContentDefined,
}

impl DeltaMode {
    pub fn fast_path(self) -> FastPath {
        match self {
            Self::Fixed => FastPath::DeltaFixed,
            Self::ContentDefined => FastPath::DeltaCdc,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeltaStats {
    pub logical_bytes: u64,
    pub reused_bytes: u64,
    pub source_bytes: u64,
    pub written_bytes: u64,
}

pub fn select(
    delta_capable: bool,
    source_size: u64,
    basis_size: u64,
    tuning: TuningSnapshot,
) -> Option<DeltaMode> {
    if !delta_capable
        || source_size < DELTA_MIN_BYTES
        || tuning.p95_latency_ms < DELTA_MIN_P95_MS
        || !similar_sizes(source_size, basis_size)
    {
        return None;
    }
    if source_size >= CDC_MIN_BYTES
        && tuning.samples >= CDC_MIN_SAMPLES
        && tuning.p95_latency_ms >= CDC_MIN_P95_MS
    {
        Some(DeltaMode::ContentDefined)
    } else {
        Some(DeltaMode::Fixed)
    }
}

fn similar_sizes(left: u64, right: u64) -> bool {
    let (small, large) = if left < right {
        (left, right)
    } else {
        (right, left)
    };
    large > 0 && u128::from(small) * 100 >= u128::from(large) * 70
}

pub fn copy_file(
    source: &Path,
    basis: &Path,
    destination: &Path,
    mode: DeltaMode,
    state: &TransferState,
    base_bytes: u64,
    allow_clone_seed: bool,
) -> std::io::Result<DeltaStats> {
    let result = copy_file_inner(
        source,
        basis,
        destination,
        mode,
        state,
        base_bytes,
        allow_clone_seed,
    );
    if let Err(error) = &result
        && error.kind() != std::io::ErrorKind::AlreadyExists
    {
        let _ = std::fs::remove_file(destination);
    }
    result
}

fn copy_file_inner(
    source: &Path,
    basis: &Path,
    destination: &Path,
    mode: DeltaMode,
    state: &TransferState,
    base_bytes: u64,
    allow_clone_seed: bool,
) -> std::io::Result<DeltaStats> {
    let source_size = source.metadata()?.len();
    let _seed = crate::native_copy::seed_file_native(basis, destination, state, allow_clone_seed)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(destination)?
        .set_len(source_size)?;
    initialize_progress(state, source, source_size, base_bytes);
    let stats = match mode {
        DeltaMode::Fixed => copy_fixed(source, basis, destination, state, base_bytes)?,
        DeltaMode::ContentDefined => {
            copy_content_defined(source, basis, destination, state, base_bytes)?
        }
    };
    if let Ok(metadata) = source.metadata() {
        std::fs::set_permissions(destination, metadata.permissions())?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(destination)?
        .sync_data()?;
    Ok(stats)
}

fn initialize_progress(state: &TransferState, source: &Path, source_size: u64, base_bytes: u64) {
    let mut progress = crate::lock_util::recover(state);
    progress.current_file = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    progress.current_file_size = source_size;
    progress.current_file_copied = 0;
    progress.copied_bytes = base_bytes;
    progress.delta_reused_bytes = 0;
    progress.delta_source_bytes = 0;
}

fn copy_fixed(
    source: &Path,
    basis: &Path,
    destination: &Path,
    state: &TransferState,
    base_bytes: u64,
) -> std::io::Result<DeltaStats> {
    let mut source = std::fs::File::open(source)?;
    let mut basis = std::fs::File::open(basis)?;
    let mut destination = std::fs::OpenOptions::new().write(true).open(destination)?;
    let mut source_buffer = vec![0_u8; FIXED_BLOCK_SIZE];
    let mut basis_buffer = vec![0_u8; FIXED_BLOCK_SIZE];
    let mut stats = DeltaStats::default();
    loop {
        check_cancelled(state)?;
        let read = source.read(&mut source_buffer)?;
        if read == 0 {
            break;
        }
        let basis_read = read_up_to(&mut basis, &mut basis_buffer[..read])?;
        if basis_read == read && source_buffer[..read] == basis_buffer[..read] {
            stats.reused_bytes = stats.reused_bytes.saturating_add(read as u64);
        } else {
            destination.seek(SeekFrom::Start(stats.logical_bytes))?;
            destination.write_all(&source_buffer[..read])?;
            stats.source_bytes = stats.source_bytes.saturating_add(read as u64);
            stats.written_bytes = stats.written_bytes.saturating_add(read as u64);
        }
        stats.logical_bytes = stats.logical_bytes.saturating_add(read as u64);
        update_progress(state, base_bytes, stats);
    }
    Ok(stats)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ChunkKey {
    length: usize,
    digest: [u8; 32],
}

fn copy_content_defined(
    source: &Path,
    basis: &Path,
    destination: &Path,
    state: &TransferState,
    base_bytes: u64,
) -> std::io::Result<DeltaStats> {
    let mut index: HashMap<ChunkKey, Vec<u64>> = HashMap::new();
    let basis_source = std::fs::File::open(basis)?;
    for chunk in
        fastcdc::v2020::StreamCDC::new(basis_source, CDC_MIN_CHUNK, CDC_AVG_CHUNK, CDC_MAX_CHUNK)
    {
        check_cancelled(state)?;
        let chunk = chunk.map_err(std::io::Error::other)?;
        let key = chunk_key(&chunk.data);
        if index.len() < MAX_CDC_INDEX_ENTRIES || index.contains_key(&key) {
            let offsets = index.entry(key).or_default();
            if offsets.len() < 4 {
                offsets.push(chunk.offset);
            }
        }
    }

    let mut basis = std::fs::File::open(basis)?;
    let mut destination = std::fs::OpenOptions::new().write(true).open(destination)?;
    let source = std::fs::File::open(source)?;
    let mut stats = DeltaStats::default();
    for chunk in fastcdc::v2020::StreamCDC::new(source, CDC_MIN_CHUNK, CDC_AVG_CHUNK, CDC_MAX_CHUNK)
    {
        check_cancelled(state)?;
        let chunk = chunk.map_err(std::io::Error::other)?;
        let key = chunk_key(&chunk.data);
        let matched = matching_basis_chunk(
            &mut basis,
            index.get(&key).map(Vec::as_slice).unwrap_or_default(),
            &chunk.data,
        )?;
        if let Some((basis_offset, basis_bytes)) = matched {
            stats.reused_bytes = stats.reused_bytes.saturating_add(chunk.length as u64);
            if basis_offset != chunk.offset {
                destination.seek(SeekFrom::Start(chunk.offset))?;
                destination.write_all(&basis_bytes)?;
                stats.written_bytes = stats.written_bytes.saturating_add(chunk.length as u64);
            }
        } else {
            destination.seek(SeekFrom::Start(chunk.offset))?;
            destination.write_all(&chunk.data)?;
            stats.source_bytes = stats.source_bytes.saturating_add(chunk.length as u64);
            stats.written_bytes = stats.written_bytes.saturating_add(chunk.length as u64);
        }
        stats.logical_bytes = chunk.offset.saturating_add(chunk.length as u64);
        update_progress(state, base_bytes, stats);
    }
    Ok(stats)
}

fn chunk_key(data: &[u8]) -> ChunkKey {
    ChunkKey {
        length: data.len(),
        digest: *blake3::hash(data).as_bytes(),
    }
}

fn matching_basis_chunk(
    basis: &mut std::fs::File,
    offsets: &[u64],
    expected: &[u8],
) -> std::io::Result<Option<(u64, Vec<u8>)>> {
    let mut buffer = vec![0_u8; expected.len()];
    for offset in offsets {
        basis.seek(SeekFrom::Start(*offset))?;
        if read_up_to(basis, &mut buffer)? == expected.len() && buffer == expected {
            return Ok(Some((*offset, buffer)));
        }
    }
    Ok(None)
}

fn read_up_to(reader: &mut std::fs::File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = reader.read(&mut buffer[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

fn check_cancelled(state: &TransferState) -> std::io::Result<()> {
    if crate::lock_util::recover(state).cancelled {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ))
    } else {
        Ok(())
    }
}

fn update_progress(state: &TransferState, base_bytes: u64, stats: DeltaStats) {
    let mut progress = crate::lock_util::recover(state);
    progress.current_file_copied = stats.logical_bytes;
    progress.copied_bytes = base_bytes.saturating_add(stats.logical_bytes);
    progress.delta_reused_bytes = stats.reused_bytes;
    progress.delta_source_bytes = stats.source_bytes;
    progress.maybe_sample();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::sync::{Arc, Mutex};

    fn state(total: u64) -> TransferState {
        Arc::new(Mutex::new(crate::transfer::TransferProgress::new(total, 1)))
    }

    #[test]
    fn policy_requires_size_similarity_latency_and_measurements_for_cdc() {
        let fixed = TuningSnapshot {
            p95_latency_ms: 35.0,
            samples: 0,
            ..Default::default()
        };
        assert_eq!(
            select(true, DELTA_MIN_BYTES, DELTA_MIN_BYTES, fixed),
            Some(DeltaMode::Fixed)
        );
        assert_eq!(select(false, DELTA_MIN_BYTES, DELTA_MIN_BYTES, fixed), None);
        assert_eq!(
            select(true, DELTA_MIN_BYTES - 1, DELTA_MIN_BYTES, fixed),
            None
        );
        assert_eq!(
            select(true, DELTA_MIN_BYTES, DELTA_MIN_BYTES * 2, fixed),
            None
        );

        let cdc = TuningSnapshot {
            p95_latency_ms: CDC_MIN_P95_MS,
            samples: CDC_MIN_SAMPLES,
            ..Default::default()
        };
        assert_eq!(
            select(true, CDC_MIN_BYTES, CDC_MIN_BYTES, cdc),
            Some(DeltaMode::ContentDefined)
        );
    }

    #[test]
    fn fixed_delta_rewrites_only_changed_blocks() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis.bin");
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("staging.bin");
        let mut bytes = vec![7_u8; FIXED_BLOCK_SIZE * 3];
        std::fs::write(&basis, &bytes).unwrap();
        bytes[FIXED_BLOCK_SIZE + 5] = 9;
        std::fs::write(&source, &bytes).unwrap();

        let stats = copy_file(
            &source,
            &basis,
            &destination,
            DeltaMode::Fixed,
            &state(bytes.len() as u64),
            0,
            true,
        )
        .unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), bytes);
        assert_eq!(stats.reused_bytes, (FIXED_BLOCK_SIZE * 2) as u64);
        assert_eq!(stats.source_bytes, FIXED_BLOCK_SIZE as u64);
    }

    #[test]
    fn cdc_delta_reuses_chunks_after_an_insertion() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis.bin");
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("staging.bin");
        let mut random = 0x4d59_5df4_d0f3_3173_u64;
        let basis_bytes = (0..(8 * 1024 * 1024))
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                random as u8
            })
            .collect::<Vec<_>>();
        let mut source_bytes = vec![42_u8; 32 * 1024];
        source_bytes.extend_from_slice(&basis_bytes);
        std::fs::write(&basis, &basis_bytes).unwrap();
        std::fs::write(&source, &source_bytes).unwrap();

        let stats = copy_file(
            &source,
            &basis,
            &destination,
            DeltaMode::ContentDefined,
            &state(source_bytes.len() as u64),
            0,
            true,
        )
        .unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), source_bytes);
        assert!(stats.reused_bytes > CDC_AVG_CHUNK as u64, "{stats:?}");
        assert!(stats.source_bytes < stats.logical_bytes, "{stats:?}");
    }
}
