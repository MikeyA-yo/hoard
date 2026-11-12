//! NTFS fast path: build the tree from the volume's Master File Table.
//!
//! Instead of listing directories and querying every entry, the whole `$MFT`
//! is read sequentially in large chunks and each file record is parsed
//! directly (the approach of tools like WizTree). This needs a raw volume
//! handle, so it only runs when Hoard is elevated; [`super::scan`] falls back
//! to the normal walk whenever anything here is unavailable or fails.
//!
//! The resulting tree carries the same identities and sizes the normal walk
//! records (volume serial, file reference number, allocated and apparent
//! size, last write time), so cleanup revalidation works unchanged.
//!
//! The on-disk MFT can lag the in-memory state by a few seconds (NTFS writes
//! metadata lazily). Cleanup re-checks every item against the live
//! filesystem, so a stale entry is skipped, never removed by mistake.

mod record;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use super::{ScanError, ScanErrorKind, ScanEvent, ScanOptions, WalkState};
use crate::platform::{self, EntryMeta, VolumeRead, VOLUME_ALIGN};
use crate::size::SizeMode;
use crate::tree::{NodeFlags, NodeKind, Tree, TreeBuilder};
use record::{
    apply_fixups, parse_record, BootSector, Record, Run, FILE_ATTRIBUTE_COMPRESSED,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE, NAMESPACE_DOS, RECORD_NUMBER_MASK,
};

/// Reparse tags with this bit set are links (symlinks, junctions, mount
/// points); see `platform::windows`.
const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;

/// Record number of the volume's root directory.
const ROOT_RECORD: u64 = 5;

/// Records below this are NTFS metadata files (`$MFT`, `$LogFile`,
/// `$Extend`, ...), which a directory listing never shows.
const FIRST_USER_RECORD: u64 = 16;

/// Bytes per MFT read, in MiB (tunable with `HOARD_MFT_CHUNK_MB`).
const CHUNK_MB: u64 = 2;

/// Concurrent MFT reads (tunable with `HOARD_MFT_READERS`). NVMe drives
/// deliver much more with several requests in flight; a SATA SSD measured
/// the same with 1 to 16 (see `docs/benchmarks.md`). With [`CHUNK_MB`] this
/// keeps about 50 MB of buffers in flight.
const READERS: usize = 8;

/// A diagnostic tuning knob read from the environment, clamped to `range`.
fn tuning(var: &str, default: u64, range: std::ops::RangeInclusive<u64>) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(default, |v| v.clamp(*range.start(), *range.end()))
}

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
const FILETIME_UNIX_OFFSET_SECS: i64 = 11_644_473_600;

/// Why the fast path could not produce a tree. The caller logs it and walks
/// instead.
#[derive(Debug)]
pub(crate) enum MftError {
    Io(io::Error),
    Format(String),
    /// The scan root's record does not match the live filesystem.
    RootMismatch,
    Cancelled,
}

impl std::fmt::Display for MftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MftError::Io(e) => write!(f, "reading the volume failed: {e}"),
            MftError::Format(m) => write!(f, "unexpected NTFS structure: {m}"),
            MftError::RootMismatch => f.write_str("the scan root is not where the MFT says"),
            MftError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl From<io::Error> for MftError {
    fn from(e: io::Error) -> Self {
        MftError::Io(e)
    }
}

impl From<record::ParseError> for MftError {
    fn from(e: record::ParseError) -> Self {
        MftError::Format(e.0)
    }
}

/// Merged information for one file (base record plus extension records).
#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    sequence: u16,
    in_use: bool,
    is_dir: bool,
    attrs: u32,
    mtime_ticks: u64,
    apparent: u64,
    /// Unnamed `$DATA` allocation, or the `$I30` index allocation for a
    /// directory.
    allocated: u64,
    /// Total allocated for compressed / sparse streams.
    compressed: Option<u64>,
    /// Reparse tag from a resident `$REPARSE_POINT` or, after
    /// [`resolve_reparse_tags`], from its non-resident data.
    reparse_tag: u32,
    /// First cluster of a non-resident `$REPARSE_POINT` (0 if none).
    reparse_lcn: u64,
    /// Tag duplicated in a `$FILE_NAME` (can be stale; last resort).
    name_tag: u32,
    /// Hard links: `$FILE_NAME`s outside the DOS namespace.
    links: u16,
}

/// A directory entry: `child` is listed in `parent` under a name.
#[derive(Clone, Copy, Debug)]
struct Link {
    parent: u32,
    parent_seq: u16,
    child: u32,
    name_start: u32,
    name_len: u16,
}

/// Everything read from the MFT, before tree building.
struct Table {
    entries: Vec<Entry>,
    links: Vec<Link>,
    names: Vec<u16>,
    /// Records that looked like file records but failed their checks.
    bad_records: u64,
}

/// Builds the tree for `root_path` from the MFT of the volume `vol` reads.
///
/// `root_meta` is the live `stat` of the root; its file reference must match
/// the MFT, or [`MftError::RootMismatch`] is returned.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scan_volume(
    vol: &dyn VolumeRead,
    root_path: &Path,
    root_meta: &EntryMeta,
    opts: &ScanOptions,
    size_mode: SizeMode,
    progress: Sender<ScanEvent>,
    cancel: &AtomicBool,
    started: Instant,
) -> Result<Tree, MftError> {
    let mut state = WalkState::new(size_mode, progress, started);
    let t = Instant::now();
    let (mut table, boot) = read_table(vol, cancel, &mut state, root_path)?;
    let read_ms = t.elapsed().as_millis() as u64;
    let t = Instant::now();
    resolve_reparse_tags(vol, &boot, &mut table);
    tracing::debug!(
        records = table.entries.len(),
        links = table.links.len(),
        bad = table.bad_records,
        read_ms,
        reparse_ms = t.elapsed().as_millis() as u64,
        "MFT read"
    );
    // Progress counted records while reading; count entries from here on.
    state.files = 0;
    let root_record = root_meta.ino & RECORD_NUMBER_MASK;
    let root_seq = (root_meta.ino >> 48) as u16;
    let root_ok = table
        .entries
        .get(usize::try_from(root_record).unwrap_or(usize::MAX))
        .is_some_and(|e| e.in_use && e.is_dir && e.sequence == root_seq);
    if !root_ok {
        return Err(MftError::RootMismatch);
    }
    let t = Instant::now();
    let tree = build_tree(
        &table, root_path, root_meta, opts, size_mode, cancel, &mut state,
    );
    tracing::debug!(build_ms = t.elapsed().as_millis() as u64, "MFT tree built");
    tree
}

/// One read of the MFT: `len` bytes at volume offset `offset` (or zeros for
/// a sparse run), holding `records` records starting at `first`.
struct Chunk {
    offset: Option<u64>,
    len: u64,
    first: u64,
    records: u64,
}

/// An aligned read buffer: `data[start..]` is aligned to [`VOLUME_ALIGN`].
struct AlignedBuf {
    data: Vec<u8>,
    start: usize,
}

impl AlignedBuf {
    fn with_len(mut self, len: usize) -> Self {
        if self.data.len() < len + VOLUME_ALIGN {
            self.data = vec![0u8; len + VOLUME_ALIGN];
            self.start = self.data.as_ptr().align_offset(VOLUME_ALIGN);
        }
        self
    }

    fn slice(&mut self, len: usize) -> &mut [u8] {
        &mut self.data[self.start..self.start + len]
    }
}

/// A filled chunk on its way from a reader thread: buffer, first record
/// number, record count.
type Filled = io::Result<(AlignedBuf, u64, u64)>;

/// Reads and parses the whole MFT. Reader threads keep several reads in
/// flight while the calling thread parses filled chunks in parallel (rayon)
/// and folds the results into the table. Chunks may arrive in any order;
/// merging does not depend on it.
fn read_table(
    vol: &dyn VolumeRead,
    cancel: &AtomicBool,
    state: &mut WalkState,
    root_path: &Path,
) -> Result<(Table, BootSector), MftError> {
    use rayon::prelude::*;

    let mut boot_buf = vec![0u8; 4096];
    vol.read_at(0, &mut boot_buf)?;
    let boot = BootSector::parse(&boot_buf)?;
    let (runs, mft_bytes) = mft_runs(vol, &boot)?;
    let record_size = boot.record_size;
    let record_count = mft_bytes / record_size;
    let count = usize::try_from(record_count)
        .ok()
        .filter(|&n| n <= u32::MAX as usize)
        .ok_or_else(|| MftError::Format("MFT too large".into()))?;

    let chunk_bytes = tuning("HOARD_MFT_CHUNK_MB", CHUNK_MB, 1..=64) << 20;
    let readers = tuning("HOARD_MFT_READERS", READERS as u64, 1..=16) as usize;

    // Plan the reads: whole records, sector-aligned, never past the MFT.
    let mut chunks = Vec::new();
    let mut first = 0u64;
    for run in &runs {
        let mut run_bytes = run.clusters * boot.cluster_size;
        let mut offset = 0u64;
        while run_bytes >= record_size && first < record_count {
            let want = (run_bytes.min(chunk_bytes) / record_size * record_size)
                .min((record_count - first) * record_size);
            chunks.push(Chunk {
                offset: run.lcn.map(|lcn| lcn * boot.cluster_size + offset),
                len: want.div_ceil(boot.bytes_per_sector) * boot.bytes_per_sector,
                first,
                records: want / record_size,
            });
            first += want / record_size;
            run_bytes -= want;
            offset += want;
        }
    }

    let mut table = Table {
        entries: vec![Entry::default(); count],
        links: Vec::with_capacity(count),
        names: Vec::with_capacity(count * 16),
        bad_records: 0,
    };
    let rs = record_size as usize;

    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| -> Result<(), MftError> {
        let (full_tx, full_rx) = crossbeam_channel::bounded::<Filled>(readers * 2);
        let (free_tx, free_rx) = crossbeam_channel::unbounded::<AlignedBuf>();
        let (chunks, next) = (&chunks, &next);
        let mut handles = Vec::new();
        for _ in 0..readers {
            let (full_tx, free_rx) = (full_tx.clone(), free_rx.clone());
            handles.push(scope.spawn(move || {
                // Time spent in reads, and blocked handing chunks over.
                let (mut io, mut blocked) = (Duration::ZERO, Duration::ZERO);
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(c) = chunks.get(i) else { break };
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let fresh = || AlignedBuf {
                        data: Vec::new(),
                        start: 0,
                    };
                    let mut buf = free_rx.try_recv().unwrap_or_else(|_| fresh());
                    buf = buf.with_len(c.len as usize);
                    let t = Instant::now();
                    let read = match c.offset {
                        Some(off) => vol.read_at(off, buf.slice(c.len as usize)),
                        None => {
                            buf.slice(c.len as usize).fill(0);
                            Ok(())
                        }
                    };
                    io += t.elapsed();
                    let failed = read.is_err();
                    let t = Instant::now();
                    let sent = full_tx.send(read.map(|()| (buf, c.first, c.records)));
                    blocked += t.elapsed();
                    if sent.is_err() || failed {
                        break;
                    }
                }
                (io, blocked)
            }));
        }
        drop(full_tx);
        let (mut waiting, mut parsing, mut merging) =
            (Duration::ZERO, Duration::ZERO, Duration::ZERO);
        let mut done_records = 0u64;
        let mut t = Instant::now();
        for msg in full_rx {
            waiting += t.elapsed();
            if cancel.load(Ordering::Relaxed) {
                return Err(MftError::Cancelled);
            }
            let (mut buf, first, records) = msg?;
            let used = records as usize * rs;
            let t_parse = Instant::now();
            let parsed: Vec<Parsed> = buf.slice(used).par_chunks_mut(rs).map(parse_raw).collect();
            parsing += t_parse.elapsed();
            let t_merge = Instant::now();
            for (i, p) in parsed.into_iter().enumerate() {
                merge(&mut table, first + i as u64, p);
            }
            merging += t_merge.elapsed();
            let _ = free_tx.send(buf);
            done_records += records;
            if state.progress_throttle.ready(Instant::now()) {
                state.files = done_records;
                state.emit_progress(root_path, 0);
            }
            t = Instant::now();
        }
        let (mut io, mut blocked) = (Duration::ZERO, Duration::ZERO);
        for h in handles {
            let (a, b) = h.join().unwrap_or_default();
            io += a;
            blocked += b;
        }
        tracing::debug!(
            readers,
            chunk_mb = chunk_bytes >> 20,
            chunks = chunks.len(),
            mft_runs = runs.len(),
            io_ms_all_readers = io.as_millis() as u64,
            readers_blocked_ms = blocked.as_millis() as u64,
            parser_waiting_ms = waiting.as_millis() as u64,
            parse_ms = parsing.as_millis() as u64,
            merge_ms = merging.as_millis() as u64,
            "MFT read phases"
        );
        Ok(())
    })?;
    if cancel.load(Ordering::Relaxed) {
        return Err(MftError::Cancelled);
    }
    Ok((table, boot))
}

/// Reads the tag of every reparse point whose `$REPARSE_POINT` attribute is
/// non-resident (long junction or symlink targets). Few entries need this.
fn resolve_reparse_tags(vol: &dyn VolumeRead, boot: &BootSector, table: &mut Table) {
    let mut buf = vec![0u8; boot.bytes_per_sector as usize];
    for e in table.entries.iter_mut() {
        if !e.in_use
            || e.attrs & FILE_ATTRIBUTE_REPARSE_POINT == 0
            || e.reparse_tag != 0
            || e.reparse_lcn == 0
        {
            continue;
        }
        if vol
            .read_at(e.reparse_lcn * boot.cluster_size, &mut buf)
            .is_ok()
        {
            e.reparse_tag = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        }
    }
}

/// Locates `$MFT` itself: its data runs and size, from record 0 plus any
/// extension records its attribute list names.
fn mft_runs(vol: &dyn VolumeRead, boot: &BootSector) -> Result<(Vec<Run>, u64), MftError> {
    let read_at_lcn = |vol: &dyn VolumeRead, byte: u64| -> Result<Vec<u8>, MftError> {
        let rs = boot.record_size;
        let sector = boot.bytes_per_sector;
        let start = byte / sector * sector;
        let len = (byte + rs - start).div_ceil(sector) * sector;
        let mut buf = vec![0u8; len as usize];
        vol.read_at(start, &mut buf)?;
        let off = (byte - start) as usize;
        let mut rec = buf[off..off + rs as usize].to_vec();
        if !apply_fixups(&mut rec) {
            return Err(MftError::Format("torn $MFT record".into()));
        }
        Ok(rec)
    };
    let rec0 = read_at_lcn(vol, boot.mft_lcn * boot.cluster_size)?;
    let first = parse_record(&rec0, true)
        .filter(|r| r.in_use)
        .ok_or_else(|| MftError::Format("bad $MFT record".into()))?;
    let sizes = first
        .data
        .ok_or_else(|| MftError::Format("$MFT has no data".into()))?;
    let mut segments = first.data_runs.clone();
    // Runs of record 0 locate any extension record that holds more of them.
    for seg in first.data_segments.iter().filter(|&&r| r != 0) {
        let byte_in_mft = seg * boot.record_size;
        let lcn_byte = locate(&segments, boot.cluster_size, byte_in_mft)
            .ok_or_else(|| MftError::Format("$MFT extension outside known runs".into()))?;
        let rec = read_at_lcn(vol, lcn_byte)?;
        let ext = parse_record(&rec, true)
            .ok_or_else(|| MftError::Format("bad $MFT extension record".into()))?;
        segments.extend(ext.data_runs);
    }
    segments.sort_by_key(|(vcn, _)| *vcn);
    let mut runs = Vec::new();
    let mut next_vcn = 0u64;
    for (vcn, seg_runs) in segments {
        if vcn != next_vcn {
            return Err(MftError::Format("gap in $MFT runs".into()));
        }
        next_vcn += seg_runs.iter().map(|r| r.clusters).sum::<u64>();
        runs.extend(seg_runs);
    }
    if next_vcn * boot.cluster_size < sizes.apparent {
        return Err(MftError::Format("$MFT runs shorter than its size".into()));
    }
    Ok((runs, sizes.apparent))
}

/// Maps a byte offset within a run list to a volume byte offset.
fn locate(segments: &[(u64, Vec<Run>)], cluster: u64, byte: u64) -> Option<u64> {
    let target_vcn = byte / cluster;
    for (start, runs) in segments {
        let mut vcn = *start;
        for r in runs {
            if (vcn..vcn + r.clusters).contains(&target_vcn) {
                let lcn = r.lcn?;
                return Some((lcn + (target_vcn - vcn)) * cluster + byte % cluster);
            }
            vcn += r.clusters;
        }
    }
    None
}

/// The outcome of parsing one raw record.
enum Parsed {
    /// Never used, zeroed, or free.
    Empty,
    /// Looked like a file record but failed its checks.
    Bad,
    Ok(Record),
}

/// Parses one raw record (fixups applied in place). Runs in parallel.
fn parse_raw(rec: &mut [u8]) -> Parsed {
    if rec.get(0..4) != Some(b"FILE".as_slice()) {
        return Parsed::Empty;
    }
    if !apply_fixups(rec) {
        return Parsed::Bad;
    }
    match parse_record(rec, false) {
        Some(r) if r.in_use => Parsed::Ok(r),
        Some(_) => Parsed::Empty,
        None => Parsed::Bad,
    }
}

/// Folds one parsed record into the table (its own entry, or its base
/// record's entry for an extension record).
fn merge(table: &mut Table, record_no: u64, parsed: Parsed) {
    let r = match parsed {
        Parsed::Empty => return,
        Parsed::Bad => {
            table.bad_records += 1;
            return;
        }
        Parsed::Ok(r) => r,
    };
    let owner = r.base.unwrap_or(record_no);
    let Some(owner_idx) = usize::try_from(owner)
        .ok()
        .filter(|&i| i < table.entries.len())
    else {
        table.bad_records += 1;
        return;
    };
    let Record {
        sequence,
        is_dir,
        base,
        standard,
        names,
        data,
        index_allocated,
        reparse_tag,
        reparse_lcn,
        ..
    } = r;
    let e = &mut table.entries[owner_idx];
    if base.is_none() {
        e.sequence = sequence;
        e.in_use = true;
        e.is_dir = is_dir;
    }
    if let Some((attrs, mtime)) = standard {
        e.attrs = attrs;
        e.mtime_ticks = mtime;
    }
    if let Some(d) = data {
        e.apparent = d.apparent;
        e.allocated = d.allocated;
        e.compressed = d.compressed;
    }
    if let Some(a) = index_allocated {
        e.allocated = a;
    }
    if let Some(tag) = reparse_tag {
        e.reparse_tag = tag;
    }
    if let Some(lcn) = reparse_lcn {
        e.reparse_lcn = lcn;
    }
    for n in names {
        if n.reparse_tag != 0 {
            e.name_tag = n.reparse_tag;
        }
        if n.namespace == NAMESPACE_DOS {
            continue;
        }
        e.links = e.links.saturating_add(1);
        let (Ok(parent), Ok(child), Ok(name_start), Ok(name_len)) = (
            u32::try_from(n.parent & RECORD_NUMBER_MASK),
            u32::try_from(owner),
            u32::try_from(table.names.len()),
            u16::try_from(n.name.len()),
        ) else {
            continue;
        };
        table.names.extend_from_slice(&n.name);
        table.links.push(Link {
            parent,
            parent_seq: (n.parent >> 48) as u16,
            child,
            name_start,
            name_len,
        });
    }
}

/// What a scan needs to know about one entry, in `stat` terms.
fn entry_meta(e: &Entry, record: u64, dev: u64) -> EntryMeta {
    let is_link = if e.attrs & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        false
    } else {
        let tag = if e.reparse_tag != 0 {
            e.reparse_tag
        } else {
            e.name_tag
        };
        // A directory reparse point whose tag could not be read is treated
        // as a link: never descending is the safe side.
        tag & REPARSE_TAG_NAME_SURROGATE != 0 || (tag == 0 && e.is_dir)
    };
    let kind = if is_link {
        NodeKind::Symlink
    } else if e.is_dir {
        NodeKind::Dir
    } else {
        NodeKind::File
    };
    let (size_apparent, size_allocated) = match kind {
        NodeKind::Symlink => (0, 0),
        NodeKind::Dir => (0, e.allocated),
        NodeKind::File | NodeKind::Other => {
            let packed = e.attrs & (FILE_ATTRIBUTE_COMPRESSED | FILE_ATTRIBUTE_SPARSE_FILE) != 0;
            let allocated = match e.compressed {
                Some(c) if packed => c,
                _ => e.allocated,
            };
            (e.apparent, allocated)
        }
    };
    EntryMeta {
        kind,
        size_apparent,
        size_allocated,
        mtime: (e.mtime_ticks / 10_000_000) as i64 - FILETIME_UNIX_OFFSET_SECS,
        dev,
        ino: (u64::from(e.sequence) << 48) | record,
        nlink: u64::from(e.links.max(1)),
    }
}

/// One directory being emitted in depth-first order.
struct Frame {
    node: crate::tree::NodeId,
    /// Range of `order` holding this directory's links.
    next: usize,
    end: usize,
    depth: usize,
}

fn build_tree(
    table: &Table,
    root_path: &Path,
    root_meta: &EntryMeta,
    opts: &ScanOptions,
    size_mode: SizeMode,
    cancel: &AtomicBool,
    state: &mut WalkState,
) -> Result<Tree, MftError> {
    let n = table.entries.len();
    // Group links by parent (counting sort): order[start[p]..start[p+1]].
    let mut start = vec![0u32; n + 1];
    for l in &table.links {
        if let Some(c) = start.get_mut(l.parent as usize + 1) {
            *c += 1;
        }
    }
    for i in 0..n {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut order = vec![0u32; table.links.len()];
    for (i, l) in table.links.iter().enumerate() {
        if let Some(slot) = fill.get_mut(l.parent as usize) {
            order[*slot as usize] = i as u32;
            *slot += 1;
        }
    }

    let skipped: Vec<PathBuf> = opts
        .skip
        .iter()
        .filter_map(|p| std::path::absolute(p).ok())
        .collect();
    let mut path = root_path.to_path_buf();

    let dev = root_meta.dev;
    let mut builder = TreeBuilder::new(root_path.to_path_buf(), root_meta, size_mode);
    let root_record = root_meta.ino & RECORD_NUMBER_MASK;
    let range = |rec: u64| {
        (
            start[rec as usize] as usize,
            start[rec as usize + 1] as usize,
        )
    };
    let (s, e) = range(root_record);
    let mut stack = vec![Frame {
        node: builder.root(),
        next: s,
        end: e,
        depth: 0,
    }];
    let mut complete = true;

    while let Some(frame) = stack.last_mut() {
        if frame.next >= frame.end {
            stack.pop();
            if !skipped.is_empty() && !stack.is_empty() {
                path.pop();
            }
            continue;
        }
        let link = table.links[order[frame.next] as usize];
        frame.next += 1;
        let parent_node = frame.node;
        let depth = frame.depth + 1;
        let parent_record = u64::from(link.parent);
        let child = u64::from(link.child);
        let parent_seq = table.entries[parent_record as usize].sequence;
        // Stale links (parent record reused), self links (the root names
        // itself) and NTFS metadata files are not directory entries.
        if link.parent_seq != parent_seq
            || child == parent_record
            || (parent_record == ROOT_RECORD && child < FIRST_USER_RECORD)
        {
            continue;
        }
        let Some(entry) = table.entries.get(child as usize).filter(|e| e.in_use) else {
            continue;
        };
        if cancel.load(Ordering::Relaxed) {
            complete = false;
            break;
        }
        let name_units = &table.names
            [link.name_start as usize..link.name_start as usize + usize::from(link.name_len)];
        let name = platform::os_from_wide(name_units);
        let meta = entry_meta(entry, child, dev);
        let descend = meta.kind == NodeKind::Dir;
        let mut flags = NodeFlags::NONE;
        let mut child_path = None;
        if descend && !skipped.is_empty() {
            let p = path.join(&name);
            if skipped.contains(&p) {
                flags.insert(NodeFlags::SKIPPED);
            }
            child_path = Some(p);
        }
        let Some(id) = builder.push(parent_node, name.clone(), &meta, flags) else {
            complete = false;
            break;
        };
        state.record(&builder, id, depth, descend, &name);
        if state.tick_due() {
            state.emit_progress(root_path, 0);
            state.maybe_emit_batch(false);
        }
        if descend && !flags.contains(NodeFlags::SKIPPED) {
            let (s, e) = range(child);
            if let Some(p) = child_path {
                path = p;
            }
            stack.push(Frame {
                node: id,
                next: s,
                end: e,
                depth,
            });
        }
    }

    let mut errors = Vec::new();
    if table.bad_records > 0 {
        errors.push(ScanError {
            path: root_path.to_path_buf(),
            kind: ScanErrorKind::Entry,
            message: format!(
                "{} MFT records could not be read; their files may be missing",
                table.bad_records
            ),
        });
    }
    if cancel.load(Ordering::Relaxed) {
        complete = false;
    }
    state.emit_progress(root_path, errors.len());
    state.maybe_emit_batch(true);
    let mut tree = builder.finish(errors, complete);
    tree.set_source(crate::tree::ScanSource::Mft);
    Ok(tree)
}

#[cfg(test)]
mod tests;
