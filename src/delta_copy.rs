//! Fixed-block and `FastCDC` delta copy into an isolated staging file.

use crate::transfer::TransferState;
use crate::transfer_tuning::{FastPath, TuningSnapshot};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const FIXED_BLOCK_SIZE: usize = 1024 * 1024;
const CDC_MIN_CHUNK: usize = 64 * 1024;
const CDC_AVG_CHUNK: usize = 256 * 1024;
const CDC_MAX_CHUNK: usize = 1024 * 1024;
const MAX_CDC_INDEX_ENTRIES: usize = 262_144;
const CHECKPOINT_INTERVAL: u64 = 16 * 1024 * 1024;

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
    pub const fn fast_path(self) -> FastPath {
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

struct CheckpointDigests {
    source: blake3::Hasher,
    destination: blake3::Hasher,
    verified_offset: u64,
}

impl CheckpointDigests {
    /// Start from `resume` (or the beginning of the file), proving that the
    /// source and staging prefixes both still hash to the checkpoint's
    /// recorded digest. This is the resume's at-use proof: the backend does
    /// not hash the prefixes again before calling the delta copy.
    fn new(
        source: &Path,
        destination: &Path,
        resume: Option<DeltaResume>,
    ) -> std::io::Result<Self> {
        let offset = resume.map_or(0, |resume| resume.offset);
        let mut digests = Self {
            source: blake3::Hasher::new(),
            destination: blake3::Hasher::new(),
            verified_offset: 0,
        };
        hash_range_into(source, 0, offset, &mut digests.source)?;
        hash_range_into(destination, 0, offset, &mut digests.destination)?;
        let source_digest = *digests.source.finalize().as_bytes();
        if source_digest != *digests.destination.finalize().as_bytes() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "delta checkpoint prefix differs between source and staging",
            ));
        }
        if resume.is_some_and(|resume| resume.content_digest != source_digest) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Delta checkpoint content proof does not match source and staging",
            ));
        }
        digests.verified_offset = offset;
        Ok(digests)
    }

    fn update_source(&mut self, bytes: &[u8]) {
        self.source.update(bytes);
    }

    fn checkpoint(
        &mut self,
        destination: &Path,
        offset: u64,
        verify_complete: bool,
    ) -> std::io::Result<[u8; 32]> {
        if offset < self.verified_offset {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "delta checkpoint offset moved backwards",
            ));
        }
        if verify_complete {
            self.destination = blake3::Hasher::new();
            hash_range_into(destination, 0, offset, &mut self.destination)?;
        } else {
            hash_range_into(
                destination,
                self.verified_offset,
                offset,
                &mut self.destination,
            )?;
        }
        let source = *self.source.clone().finalize().as_bytes();
        if source != *self.destination.clone().finalize().as_bytes() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "delta staging bytes differ from the processed source prefix",
            ));
        }
        self.verified_offset = offset;
        Ok(source)
    }
}

struct CheckpointWriter<'a> {
    digests: &'a mut CheckpointDigests,
    publish: &'a mut dyn FnMut(u64, [u8; 32]) -> std::io::Result<()>,
    limiter: crate::transfer_tuning::BandwidthLimiter,
}

impl CheckpointWriter<'_> {
    /// Hold the copy to the volume's bandwidth limit after `bytes` more
    /// source bytes. The throttle sleep watches the cancel flag; a cancel
    /// during it checkpoints `offset` like a cancel between blocks.
    fn throttle(
        &mut self,
        bytes: usize,
        state: &TransferState,
        destination: &std::fs::File,
        destination_path: &Path,
        offset: u64,
    ) -> std::io::Result<()> {
        if self.limiter.consume(bytes, || is_cancelled(state)).is_err() {
            self.sync(destination, destination_path, offset, false)?;
            return Err(interrupted());
        }
        Ok(())
    }

    fn publish_initial(&mut self) -> std::io::Result<()> {
        let digest = *self.digests.source.clone().finalize().as_bytes();
        (self.publish)(0, digest)
    }

    fn sync(
        &mut self,
        destination: &std::fs::File,
        destination_path: &Path,
        offset: u64,
        verify_complete: bool,
    ) -> std::io::Result<()> {
        destination.sync_data()?;
        let digest = self
            .digests
            .checkpoint(destination_path, offset, verify_complete)?;
        (self.publish)(offset, digest)
    }
}

/// Where a resumed delta copy continues: the checkpoint offset and the
/// BLAKE3 proof of the first `offset` bytes recorded with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeltaResume {
    pub offset: u64,
    pub content_digest: [u8; 32],
}

#[derive(Clone, Copy)]
pub struct DeltaRequest<'a> {
    pub source: &'a Path,
    pub basis: &'a Path,
    pub destination: &'a Path,
    pub mode: DeltaMode,
    pub state: &'a TransferState,
    pub base_bytes: u64,
    pub allow_clone_seed: bool,
    pub resume: Option<DeltaResume>,
    /// The volume's bandwidth limit, applied to every source byte processed.
    pub bandwidth: crate::transfer_tuning::VolumeRule,
    /// Re-hash the whole staging file at the end. The incremental checkpoint
    /// digests already read every staging byte once, so this extra full read
    /// (over the slow link) is kept only for verifying durability profiles.
    pub reverify_staging: bool,
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
    request: DeltaRequest<'_>,
    checkpoint: &mut dyn FnMut(u64, [u8; 32]) -> std::io::Result<()>,
) -> std::io::Result<DeltaStats> {
    // Once a checkpoint naming the staging file is published, the executor
    // owns it: a reconnect retry resumes from it, and the executor removes it
    // itself when the checkpoint turns out unusable. Deleting it here would
    // force every disconnect to restart the delta copy from scratch.
    let mut published = false;
    let mut tracked = |offset, digest| {
        checkpoint(offset, digest)?;
        published = true;
        Ok(())
    };
    let result = copy_file_inner(&request, &mut tracked);
    if let Err(error) = &result
        && !published
        && error.kind() != std::io::ErrorKind::AlreadyExists
        && error.kind() != std::io::ErrorKind::Interrupted
    {
        let _ = std::fs::remove_file(request.destination);
    }
    result
}

fn copy_file_inner(
    request: &DeltaRequest<'_>,
    checkpoint: &mut dyn FnMut(u64, [u8; 32]) -> std::io::Result<()>,
) -> std::io::Result<DeltaStats> {
    let DeltaRequest {
        source,
        basis,
        destination,
        mode,
        state,
        base_bytes,
        allow_clone_seed,
        resume,
        bandwidth,
        reverify_staging,
    } = *request;
    let resume_offset = resume.map(|resume| resume.offset);
    let source_size = source.metadata()?.len();
    let start_offset = resume_offset.unwrap_or(0);
    if start_offset > source_size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "delta checkpoint exceeds the source size",
        ));
    }
    if resume_offset.is_none() {
        // The seed copies the whole basis without byte progress; say why the
        // bar stands still on a slow link.
        let _reason = WaitingReason::show(state, PREPARING_DESTINATION);
        let _seed =
            crate::native_copy::seed_file_native(basis, destination, state, allow_clone_seed)?;
        let destination = std::fs::OpenOptions::new().write(true).open(destination)?;
        destination.set_len(source_size)?;
        destination.sync_data()?;
    } else if destination.metadata()?.len() != source_size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "delta staging size changed after checkpoint",
        ));
    }
    let mut digests = CheckpointDigests::new(source, destination, resume)?;
    let mut checkpoints = CheckpointWriter {
        digests: &mut digests,
        publish: checkpoint,
        limiter: crate::transfer_tuning::BandwidthLimiter::new(bandwidth),
    };
    if resume_offset.is_none() {
        checkpoints.publish_initial()?;
    }
    initialize_progress(state, source, source_size, base_bytes, start_offset);
    let stats = match mode {
        DeltaMode::Fixed => copy_fixed(
            source,
            basis,
            destination,
            state,
            base_bytes,
            start_offset,
            &mut checkpoints,
        )?,
        DeltaMode::ContentDefined => copy_content_defined(
            source,
            basis,
            destination,
            state,
            base_bytes,
            start_offset,
            &mut checkpoints,
        )?,
    };
    let staging = std::fs::OpenOptions::new().write(true).open(destination)?;
    checkpoints.sync(&staging, destination, source_size, reverify_staging)?;
    drop(staging);
    // Permissions last and best-effort, as in the buffered copy: a read-only
    // source mode must not lock us out of the final sync above, and remote
    // filesystems that reject chmod should not fail a complete transfer.
    if let Ok(metadata) = source.metadata() {
        let _ = std::fs::set_permissions(destination, metadata.permissions());
    }
    Ok(stats)
}

fn initialize_progress(
    state: &TransferState,
    source: &Path,
    source_size: u64,
    base_bytes: u64,
    start_offset: u64,
) {
    let mut progress = crate::lock_util::recover(state);
    progress.current_file = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    progress.current_file_size = source_size;
    progress.current_file_copied = start_offset;
    progress.copied_bytes = base_bytes.saturating_add(start_offset);
    progress.delta_reused_bytes = 0;
    progress.delta_source_bytes = 0;
}

fn copy_fixed(
    source: &Path,
    basis: &Path,
    destination: &Path,
    state: &TransferState,
    base_bytes: u64,
    start_offset: u64,
    checkpoints: &mut CheckpointWriter<'_>,
) -> std::io::Result<DeltaStats> {
    let source_size = source.metadata()?.len();
    if !start_offset.is_multiple_of(FIXED_BLOCK_SIZE as u64) && start_offset != source_size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "fixed delta checkpoint is not block-aligned",
        ));
    }
    let mut source = std::fs::File::open(source)?;
    let mut basis = std::fs::File::open(basis)?;
    let destination_path = destination;
    let mut destination = std::fs::OpenOptions::new().write(true).open(destination)?;
    source.seek(SeekFrom::Start(start_offset))?;
    basis.seek(SeekFrom::Start(start_offset))?;
    let mut source_buffer = vec![0_u8; FIXED_BLOCK_SIZE];
    let mut basis_buffer = vec![0_u8; FIXED_BLOCK_SIZE];
    let mut stats = DeltaStats {
        logical_bytes: start_offset,
        ..Default::default()
    };
    let mut checkpoint_at = next_checkpoint(start_offset);
    loop {
        if is_cancelled(state) {
            checkpoints.sync(&destination, destination_path, stats.logical_bytes, false)?;
            return Err(interrupted());
        }
        // Fill whole blocks: a short read (SMB/NFS) would leave
        // `logical_bytes` unaligned and make later checkpoints unresumable.
        let read = read_up_to(&mut source, &mut source_buffer)?;
        if read == 0 {
            break;
        }
        checkpoints.digests.update_source(&source_buffer[..read]);
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
        if stats.logical_bytes >= checkpoint_at {
            checkpoints.sync(&destination, destination_path, stats.logical_bytes, false)?;
            checkpoint_at = next_checkpoint(stats.logical_bytes);
        }
        checkpoints.throttle(
            read,
            state,
            &destination,
            destination_path,
            stats.logical_bytes,
        )?;
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
    start_offset: u64,
    checkpoints: &mut CheckpointWriter<'_>,
) -> std::io::Result<DeltaStats> {
    let mut index: HashMap<ChunkKey, Vec<u64>> = HashMap::new();
    // Indexing reads the whole basis before any byte of progress.
    let indexing = WaitingReason::show(state, INDEXING_DESTINATION);
    let basis_source = std::fs::File::open(basis)?;
    for chunk in
        fastcdc::v2020::StreamCDC::new(basis_source, CDC_MIN_CHUNK, CDC_AVG_CHUNK, CDC_MAX_CHUNK)
    {
        if is_cancelled(state) {
            return Err(interrupted());
        }
        let chunk = chunk.map_err(std::io::Error::other)?;
        let key = chunk_key(&chunk.data);
        if index.len() < MAX_CDC_INDEX_ENTRIES || index.contains_key(&key) {
            let offsets = index.entry(key).or_default();
            if offsets.len() < 4 {
                offsets.push(chunk.offset);
            }
        }
    }
    drop(indexing);

    let mut basis = std::fs::File::open(basis)?;
    let destination_path = destination;
    let mut destination = std::fs::OpenOptions::new().write(true).open(destination)?;
    let source = std::fs::File::open(source)?;
    let mut basis_buffer = Vec::with_capacity(CDC_MAX_CHUNK);
    let mut stats = DeltaStats {
        logical_bytes: start_offset,
        ..Default::default()
    };
    let mut checkpoint_at = next_checkpoint(start_offset);
    for chunk in fastcdc::v2020::StreamCDC::new(source, CDC_MIN_CHUNK, CDC_AVG_CHUNK, CDC_MAX_CHUNK)
    {
        if is_cancelled(state) {
            checkpoints.sync(&destination, destination_path, stats.logical_bytes, false)?;
            return Err(interrupted());
        }
        let chunk = chunk.map_err(std::io::Error::other)?;
        let chunk_end = chunk.offset.saturating_add(chunk.length as u64);
        if chunk_end <= start_offset {
            continue;
        }
        if chunk.offset < start_offset {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CDC checkpoint is not chunk-aligned",
            ));
        }
        checkpoints.digests.update_source(&chunk.data);
        let key = chunk_key(&chunk.data);
        let matched = matching_basis_chunk(
            &mut basis,
            chunk.offset,
            index.get(&key).map(Vec::as_slice).unwrap_or_default(),
            &chunk.data,
            &mut basis_buffer,
        )?;
        if let Some(basis_offset) = matched {
            stats.reused_bytes = stats.reused_bytes.saturating_add(chunk.length as u64);
            if basis_offset != chunk.offset {
                destination.seek(SeekFrom::Start(chunk.offset))?;
                destination.write_all(&basis_buffer)?;
                stats.written_bytes = stats.written_bytes.saturating_add(chunk.length as u64);
            }
        } else {
            destination.seek(SeekFrom::Start(chunk.offset))?;
            destination.write_all(&chunk.data)?;
            stats.source_bytes = stats.source_bytes.saturating_add(chunk.length as u64);
            stats.written_bytes = stats.written_bytes.saturating_add(chunk.length as u64);
        }
        stats.logical_bytes = chunk_end;
        update_progress(state, base_bytes, stats);
        if stats.logical_bytes >= checkpoint_at {
            checkpoints.sync(&destination, destination_path, stats.logical_bytes, false)?;
            checkpoint_at = next_checkpoint(stats.logical_bytes);
        }
        checkpoints.throttle(
            chunk.length,
            state,
            &destination,
            destination_path,
            stats.logical_bytes,
        )?;
    }
    Ok(stats)
}

fn chunk_key(data: &[u8]) -> ChunkKey {
    ChunkKey {
        length: data.len(),
        digest: *blake3::hash(data).as_bytes(),
    }
}

/// The basis offset holding `expected`, reading it into `buffer` when the
/// bytes must be relocated. A candidate at `chunk_offset` itself needs no
/// write (the staging file was seeded from the basis), so the 32-byte BLAKE3 +
/// length key is trusted there and the slow-link re-read is skipped; the
/// checkpoint digests still compare every staging byte with the source.
fn matching_basis_chunk(
    basis: &mut std::fs::File,
    chunk_offset: u64,
    offsets: &[u64],
    expected: &[u8],
    buffer: &mut Vec<u8>,
) -> std::io::Result<Option<u64>> {
    if offsets.contains(&chunk_offset) {
        return Ok(Some(chunk_offset));
    }
    buffer.resize(expected.len(), 0);
    for offset in offsets {
        basis.seek(SeekFrom::Start(*offset))?;
        if read_up_to(basis, buffer.as_mut_slice())? == expected.len()
            && buffer.as_slice() == expected
        {
            return Ok(Some(*offset));
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

/// Activity-strip text for the delta passes that read the destination
/// without advancing the byte counters.
const PREPARING_DESTINATION: &str = "Preparing the existing destination";
const INDEXING_DESTINATION: &str = "Indexing the existing destination";

/// Shows `reason` as the transfer's waiting reason until dropped. A reason
/// already shown (a mount wait, or another delta worker on the same
/// transfer) is left in place and this guard then owns nothing; drop clears
/// the reason only if this guard set it and it is still ours.
struct WaitingReason<'a> {
    state: &'a TransferState,
    reason: Option<&'static str>,
}

impl<'a> WaitingReason<'a> {
    fn show(state: &'a TransferState, reason: &'static str) -> Self {
        let mut progress = crate::lock_util::recover(state);
        let reason = if progress.waiting_reason.is_none() {
            progress.waiting_reason = Some(reason.to_string());
            Some(reason)
        } else {
            None
        };
        Self { state, reason }
    }
}

impl Drop for WaitingReason<'_> {
    fn drop(&mut self) {
        let Some(reason) = self.reason else {
            return;
        };
        let mut progress = crate::lock_util::recover(self.state);
        if progress.waiting_reason.as_deref() == Some(reason) {
            progress.waiting_reason = None;
        }
    }
}

fn is_cancelled(state: &TransferState) -> bool {
    crate::lock_util::recover(state).cancelled
}

fn interrupted() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")
}

const fn next_checkpoint(offset: u64) -> u64 {
    offset.saturating_add(CHECKPOINT_INTERVAL)
}

fn hash_range_into(
    path: &Path,
    start: u64,
    end: u64,
    hasher: &mut blake3::Hasher,
) -> std::io::Result<()> {
    if end < start {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "hash range ends before it starts",
        ));
    }
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    file.seek(SeekFrom::Start(start))?;
    let mut remaining = end - start;
    let mut buffer = vec![0_u8; 1024 * 1024];
    while remaining > 0 {
        let limit = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("bounded hash chunk fits usize");
        let read = file.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "delta checkpoint content is shorter than its offset",
            ));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(())
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
    fn waiting_reason_shows_while_held_and_keeps_foreign_reasons() {
        let progress = state(1);
        {
            let _reason = WaitingReason::show(&progress, INDEXING_DESTINATION);
            assert_eq!(
                progress.lock().unwrap().waiting_reason.as_deref(),
                Some(INDEXING_DESTINATION)
            );
        }
        assert_eq!(progress.lock().unwrap().waiting_reason, None);

        let reason = WaitingReason::show(&progress, PREPARING_DESTINATION);
        progress.lock().unwrap().waiting_reason = Some("Mount disconnected".to_string());
        drop(reason);
        assert_eq!(
            progress.lock().unwrap().waiting_reason.as_deref(),
            Some("Mount disconnected")
        );

        // A reason shown first is neither replaced nor cleared.
        drop(WaitingReason::show(&progress, INDEXING_DESTINATION));
        assert_eq!(
            progress.lock().unwrap().waiting_reason.as_deref(),
            Some("Mount disconnected")
        );
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
    fn in_place_basis_match_trusts_the_chunk_key_without_rereading() {
        let temp = TempDir::new();
        let basis_path = temp.file("basis.bin", "zzzz----");
        let mut basis = std::fs::File::open(&basis_path).unwrap();
        let mut buffer = Vec::new();
        // The key index said offset 0 holds "data"; at the chunk's own offset
        // that is trusted, so the (here deliberately different) basis bytes
        // are not read back over the slow link.
        assert_eq!(
            matching_basis_chunk(&mut basis, 0, &[0], b"data", &mut buffer).unwrap(),
            Some(0)
        );
        // A relocated candidate is still read and compared byte for byte.
        assert_eq!(
            matching_basis_chunk(&mut basis, 8, &[0, 4], b"----", &mut buffer).unwrap(),
            Some(4)
        );
        assert_eq!(buffer, b"----");
        assert_eq!(
            matching_basis_chunk(&mut basis, 8, &[0], b"data", &mut buffer).unwrap(),
            None
        );
    }

    #[test]
    fn final_staging_rehash_runs_only_when_reverifying() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis-final.bin");
        let source = temp.path().join("source-final.bin");
        let bytes = vec![5_u8; CHECKPOINT_INTERVAL as usize + FIXED_BLOCK_SIZE];
        std::fs::write(&basis, &bytes).unwrap();
        std::fs::write(&source, &bytes).unwrap();
        for reverify_staging in [false, true] {
            let destination = temp
                .path()
                .join(format!("staging-final-{reverify_staging}.bin"));
            // After the mid-file checkpoint is published, flip a byte in the
            // already verified prefix: only a full re-hash can notice it.
            let mut corrupt = |offset: u64, _digest: [u8; 32]| {
                if offset == CHECKPOINT_INTERVAL {
                    let mut file = std::fs::OpenOptions::new().write(true).open(&destination)?;
                    file.write_all(&[6])?;
                }
                Ok(())
            };
            let result = copy_file(
                DeltaRequest {
                    source: &source,
                    basis: &basis,
                    destination: &destination,
                    mode: DeltaMode::Fixed,
                    state: &state(bytes.len() as u64),
                    base_bytes: 0,
                    allow_clone_seed: true,
                    resume: None,
                    bandwidth: Default::default(),
                    reverify_staging,
                },
                &mut corrupt,
            );
            assert_eq!(result.is_err(), reverify_staging);
        }
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
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::Fixed,
                state: &state(bytes.len() as u64),
                base_bytes: 0,
                allow_clone_seed: true,
                resume: None,
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |_, _| Ok(()),
        )
        .unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), bytes);
        assert_eq!(stats.reused_bytes, (FIXED_BLOCK_SIZE * 2) as u64);
        assert_eq!(stats.source_bytes, FIXED_BLOCK_SIZE as u64);
    }

    #[test]
    fn delta_copy_honours_the_volume_bandwidth_limit() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis-throttle.bin");
        let source = temp.path().join("source-throttle.bin");
        let destination = temp.path().join("staging-throttle.bin");
        // Two blocks at the 64 KiB/s floor would take half a minute.
        std::fs::write(&basis, vec![1_u8; FIXED_BLOCK_SIZE * 2]).unwrap();
        std::fs::write(&source, vec![2_u8; FIXED_BLOCK_SIZE * 2]).unwrap();
        let progress = state((FIXED_BLOCK_SIZE * 2) as u64);
        let canceller = {
            let progress = Arc::clone(&progress);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                crate::lock_util::recover(&progress).request_cancel();
            })
        };
        let started = std::time::Instant::now();
        let mut checkpoints = Vec::new();

        let error = copy_file(
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::Fixed,
                state: &progress,
                base_bytes: 0,
                allow_clone_seed: true,
                resume: None,
                bandwidth: crate::transfer_tuning::VolumeRule {
                    max_bytes_per_second: Some(64 * 1024),
                    ..Default::default()
                },
                reverify_staging: true,
            },
            &mut |offset, _| {
                checkpoints.push(offset);
                Ok(())
            },
        )
        .unwrap_err();

        canceller.join().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "cancel waited {:?}",
            started.elapsed()
        );
        // The throttled block is checkpointed so a later resume keeps it.
        assert_eq!(checkpoints, vec![0, FIXED_BLOCK_SIZE as u64]);
    }

    #[test]
    fn delta_resume_rejects_prefixes_that_no_longer_match_the_recorded_proof() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis-proof.bin");
        let source = temp.path().join("source-proof.bin");
        let destination = temp.path().join("staging-proof.bin");
        let bytes = vec![3_u8; FIXED_BLOCK_SIZE * 2];
        std::fs::write(&basis, &bytes).unwrap();
        std::fs::write(&source, &bytes).unwrap();
        std::fs::write(&destination, &bytes).unwrap();
        // Source and staging still agree with each other, but not with the
        // digest the checkpoint recorded: both changed since the proof.
        let recorded = *blake3::hash(&vec![9_u8; FIXED_BLOCK_SIZE]).as_bytes();

        let error = copy_file(
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::Fixed,
                state: &state(bytes.len() as u64),
                base_bytes: 0,
                allow_clone_seed: true,
                resume: Some(DeltaResume {
                    offset: FIXED_BLOCK_SIZE as u64,
                    content_digest: recorded,
                }),
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |_, _| Ok(()),
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "Delta checkpoint content proof does not match source and staging"
        );
    }

    #[test]
    fn fixed_delta_resumes_an_existing_seed_at_a_block_boundary() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis-resume.bin");
        let source = temp.path().join("source-resume.bin");
        let destination = temp.path().join("staging-resume.bin");
        let basis_bytes = vec![3_u8; FIXED_BLOCK_SIZE * 3];
        let mut source_bytes = basis_bytes.clone();
        source_bytes[17] = 4;
        source_bytes[FIXED_BLOCK_SIZE + 17] = 5;
        source_bytes[FIXED_BLOCK_SIZE * 2 + 17] = 6;
        let mut partial = basis_bytes.clone();
        partial[..FIXED_BLOCK_SIZE].copy_from_slice(&source_bytes[..FIXED_BLOCK_SIZE]);
        std::fs::write(&basis, basis_bytes).unwrap();
        std::fs::write(&source, &source_bytes).unwrap();
        std::fs::write(&destination, partial).unwrap();
        let mut checkpoints = Vec::new();

        let stats = copy_file(
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::Fixed,
                state: &state(source_bytes.len() as u64),
                base_bytes: 0,
                allow_clone_seed: true,
                resume: Some(DeltaResume {
                    offset: FIXED_BLOCK_SIZE as u64,
                    content_digest: crate::transfer::prefix_digest(
                        &source,
                        FIXED_BLOCK_SIZE as u64,
                    )
                    .unwrap(),
                }),
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |offset, _| {
                checkpoints.push(offset);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), source_bytes);
        assert_eq!(stats.logical_bytes, (FIXED_BLOCK_SIZE * 3) as u64);
        assert_eq!(checkpoints.last(), Some(&stats.logical_bytes));
    }

    #[test]
    fn interrupted_delta_keeps_its_seed_and_checkpoints_zero() {
        let temp = TempDir::new();
        let basis = temp.file("basis-cancel.bin", "basis");
        let source = temp.file("source-cancel.bin", "source");
        let destination = temp.path().join("staging-cancel.bin");
        let state = state(6);
        let cancel_state = state.clone();
        let mut checkpoints = Vec::new();

        let error = copy_file(
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::Fixed,
                state: &state,
                base_bytes: 0,
                allow_clone_seed: true,
                resume: None,
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |offset, _| {
                checkpoints.push(offset);
                cancel_state.lock().unwrap().cancelled = true;
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(destination.exists());
        assert_eq!(checkpoints, vec![0, 0]);
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
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::ContentDefined,
                state: &state(source_bytes.len() as u64),
                base_bytes: 0,
                allow_clone_seed: true,
                resume: None,
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |_, _| Ok(()),
        )
        .unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), source_bytes);
        assert!(stats.reused_bytes > CDC_AVG_CHUNK as u64, "{stats:?}");
        assert!(stats.source_bytes < stats.logical_bytes, "{stats:?}");
    }

    /// Deterministic incompressible bytes, so CDC finds varied chunk cuts.
    fn noise(len: usize, mut seed: u64) -> Vec<u8> {
        (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect()
    }

    /// Delta-copy `source` over `basis` into `destination`, returning the
    /// result and every published checkpoint offset.
    fn run_delta(
        source: &Path,
        basis: &Path,
        destination: &Path,
        mode: DeltaMode,
        resume_offset: Option<u64>,
    ) -> (std::io::Result<DeltaStats>, Vec<u64>) {
        let mut checkpoints = Vec::new();
        let result = copy_file(
            DeltaRequest {
                source,
                basis,
                destination,
                mode,
                state: &state(source.metadata().unwrap().len()),
                base_bytes: 0,
                allow_clone_seed: true,
                resume: resume_offset.map(|offset| DeltaResume {
                    offset,
                    content_digest: crate::transfer::prefix_digest(source, offset).unwrap(),
                }),
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |offset, _| {
                checkpoints.push(offset);
                Ok(())
            },
        );
        (result, checkpoints)
    }

    #[test]
    fn cdc_delta_resumes_from_a_mid_file_checkpoint() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis-cdc-resume.bin");
        let source = temp.path().join("source-cdc-resume.bin");
        let basis_bytes = noise(CHECKPOINT_INTERVAL as usize + 4 * 1024 * 1024, 0x9e37);
        let mut source_bytes = basis_bytes.clone();
        // Changes on both sides of the first checkpoint.
        source_bytes[1_000_000..1_000_100].fill(1);
        let late = CHECKPOINT_INTERVAL as usize + 2 * 1024 * 1024;
        source_bytes[late..late + 100].fill(2);
        std::fs::write(&basis, &basis_bytes).unwrap();
        std::fs::write(&source, &source_bytes).unwrap();
        let (full, checkpoints) = run_delta(
            &source,
            &basis,
            &temp.path().join("staging-cdc-full.bin"),
            DeltaMode::ContentDefined,
            None,
        );
        full.unwrap();
        let resume_at = *checkpoints
            .iter()
            .find(|&&offset| offset > 0 && offset < source_bytes.len() as u64)
            .expect("a mid-file checkpoint");
        let at = resume_at as usize;
        // Staging as an interrupted run left it: the source up to the
        // checkpoint, the seeded basis after it.
        let mut partial = source_bytes[..at].to_vec();
        partial.extend_from_slice(&basis_bytes[at..]);
        let destination = temp.path().join("staging-cdc-resume.bin");
        std::fs::write(&destination, &partial).unwrap();

        let (resumed, resumed_checkpoints) = run_delta(
            &source,
            &basis,
            &destination,
            DeltaMode::ContentDefined,
            Some(resume_at),
        );

        let stats = resumed.unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), source_bytes);
        assert_eq!(stats.logical_bytes, source_bytes.len() as u64);
        // Only the part after the checkpoint is processed again.
        assert!(
            stats.reused_bytes + stats.source_bytes <= source_bytes.len() as u64 - resume_at,
            "{stats:?}"
        );
        assert_eq!(
            resumed_checkpoints.last(),
            Some(&(source_bytes.len() as u64))
        );
        assert!(!resumed_checkpoints.contains(&0), "resume republished 0");

        // An offset inside a chunk cannot be a CDC checkpoint.
        std::fs::write(&destination, &source_bytes).unwrap();
        let (misaligned, _) = run_delta(
            &source,
            &basis,
            &destination,
            DeltaMode::ContentDefined,
            Some(resume_at + 1),
        );
        let error = misaligned.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("chunk-aligned"), "{error}");
    }

    #[test]
    fn delta_staging_matches_a_source_shorter_or_longer_than_the_basis() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis-len.bin");
        let basis_bytes = noise(FIXED_BLOCK_SIZE * 3, 0x51ed);
        std::fs::write(&basis, &basis_bytes).unwrap();
        let shorter = basis_bytes[..FIXED_BLOCK_SIZE * 2 + 1234].to_vec();
        let mut longer = basis_bytes.clone();
        longer.extend_from_slice(b"appended tail");
        let cases = [
            (DeltaMode::Fixed, &shorter),
            (DeltaMode::Fixed, &longer),
            (DeltaMode::ContentDefined, &shorter),
        ];
        for (case, (mode, bytes)) in cases.into_iter().enumerate() {
            let source = temp.path().join(format!("source-len-{case}.bin"));
            let destination = temp.path().join(format!("staging-len-{case}.bin"));
            std::fs::write(&source, bytes).unwrap();

            let (result, _) = run_delta(&source, &basis, &destination, mode, None);

            let stats = result.unwrap();
            // The seeded basis tail past a shorter source must be cut off.
            assert_eq!(&std::fs::read(&destination).unwrap(), bytes, "{mode:?}");
            assert_eq!(stats.logical_bytes, bytes.len() as u64, "{mode:?}");
            assert!(stats.reused_bytes > 0, "{mode:?} {stats:?}");
        }
    }

    #[test]
    fn failed_delta_keeps_staging_once_a_checkpoint_was_published() {
        let temp = TempDir::new();
        let basis = temp.file("basis-drop.bin", "basis");
        let source = temp.file("source-drop.bin", "source");
        let destination = temp.path().join("staging-drop.bin");
        let mut calls = 0;

        let error = copy_file(
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::Fixed,
                state: &state(6),
                base_bytes: 0,
                allow_clone_seed: true,
                resume: None,
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |_, _| {
                calls += 1;
                if calls == 1 {
                    Ok(())
                } else {
                    Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset))
                }
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        assert!(destination.exists(), "checkpointed staging must survive");
    }

    #[test]
    fn failed_delta_removes_staging_without_a_checkpoint() {
        let temp = TempDir::new();
        let basis = temp.file("basis-early.bin", "basis");
        let source = temp.file("source-early.bin", "source");
        let destination = temp.path().join("staging-early.bin");

        copy_file(
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::Fixed,
                state: &state(6),
                base_bytes: 0,
                allow_clone_seed: true,
                resume: None,
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |_, _| Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
        )
        .unwrap_err();

        assert!(!destination.exists());
    }

    #[cfg(unix)]
    #[test]
    fn delta_copy_of_a_read_only_source_completes() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new();
        let basis = temp.file("basis-ro.bin", "basis");
        let source = temp.file("source-ro.bin", "source");
        let destination = temp.path().join("staging-ro.bin");
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o444)).unwrap();
        let mut modes_at_checkpoint = Vec::new();

        copy_file(
            DeltaRequest {
                source: &source,
                basis: &basis,
                destination: &destination,
                mode: DeltaMode::Fixed,
                state: &state(6),
                base_bytes: 0,
                allow_clone_seed: true,
                resume: None,
                bandwidth: Default::default(),
                reverify_staging: true,
            },
            &mut |_, _| {
                let mode = std::fs::metadata(&destination)
                    .unwrap()
                    .permissions()
                    .mode();
                modes_at_checkpoint.push(mode & 0o777);
                Ok(())
            },
        )
        .unwrap();

        // The final sync reopens staging for writing, so it must happen while
        // staging is still writable (root would mask an EACCES otherwise).
        assert!(modes_at_checkpoint.iter().all(|mode| mode & 0o200 != 0));
        assert_eq!(std::fs::read(&destination).unwrap(), b"source");
        let mode = std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o444);
    }
}
