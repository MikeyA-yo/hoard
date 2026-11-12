//! Parsing of NTFS on-disk structures: the boot sector, MFT file records
//! (with update sequence fixups), attributes and data runs.
//!
//! Everything here is pure byte parsing with bounds checks on every read, so
//! a corrupt or torn record yields `None` / an error, never a panic.

use std::fmt;

/// File attribute flags, as stored in `$STANDARD_INFORMATION`.
pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
pub(crate) const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;
pub(crate) const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x800;

const ATTR_STANDARD_INFORMATION: u32 = 0x10;
const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_INDEX_ALLOCATION: u32 = 0xA0;
const ATTR_REPARSE_POINT: u32 = 0xC0;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// Attribute flags in the attribute header.
const ATTR_FLAG_COMPRESSED: u16 = 0x0001;
const ATTR_FLAG_SPARSE: u16 = 0x8000;

const RECORD_IN_USE: u16 = 0x0001;
const RECORD_IS_DIRECTORY: u16 = 0x0002;

/// Update sequence fixups protect every 512-byte block, whatever the sector
/// size.
const FIXUP_BLOCK: usize = 512;

/// File name namespace of a DOS-only (8.3) name: an alias, not a link.
pub(crate) const NAMESPACE_DOS: u8 = 2;

/// Lower 48 bits of a file reference are the record number.
pub(crate) const RECORD_NUMBER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// A parse failure for volume-level structures. Record-level problems are
/// reported as `None` instead, so one bad record never stops a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError(pub(crate) String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// Geometry read from the NTFS boot sector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BootSector {
    pub(crate) bytes_per_sector: u64,
    pub(crate) cluster_size: u64,
    pub(crate) mft_lcn: u64,
    pub(crate) record_size: u64,
    pub(crate) serial: u64,
}

impl BootSector {
    pub(crate) fn parse(b: &[u8]) -> Result<BootSector, ParseError> {
        let err = |m: &str| ParseError(format!("boot sector: {m}"));
        if b.get(3..11) != Some(b"NTFS    ".as_slice()) {
            return Err(err("not an NTFS volume"));
        }
        let bytes_per_sector = u64::from(u16_at(b, 0x0B).ok_or_else(|| err("short"))?);
        let spc_raw = *b.get(0x0D).ok_or_else(|| err("short"))?;
        // Values above 0x80 encode 2^(256 - value) sectors (64 KiB+ clusters).
        let sectors_per_cluster: u64 = if spc_raw <= 0x80 {
            u64::from(spc_raw)
        } else {
            1u64.checked_shl(256 - u32::from(spc_raw))
                .ok_or_else(|| err("bad cluster size"))?
        };
        let cluster_size = bytes_per_sector * sectors_per_cluster;
        let mft_lcn = u64_at(b, 0x30).ok_or_else(|| err("short"))?;
        let cpr = *b.get(0x40).ok_or_else(|| err("short"))? as i8;
        // Positive: clusters per record. Negative: 2^(-n) bytes.
        let record_size = if cpr > 0 {
            cluster_size * cpr as u64
        } else {
            1u64.checked_shl(u32::from(cpr.unsigned_abs()))
                .ok_or_else(|| err("bad record size"))?
        };
        let serial = u64_at(b, 0x48).ok_or_else(|| err("short"))?;
        let valid = |v: u64| v.is_power_of_two() && (512..=1 << 21).contains(&v);
        if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096)
            || !valid(cluster_size)
            || !valid(record_size)
            || mft_lcn == 0
        {
            return Err(err("implausible geometry"));
        }
        Ok(BootSector {
            bytes_per_sector,
            cluster_size,
            mft_lcn,
            record_size,
            serial,
        })
    }
}

/// One contiguous run of a non-resident attribute. `lcn` is `None` for a
/// sparse run (no clusters allocated).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    pub(crate) lcn: Option<u64>,
    pub(crate) clusters: u64,
}

/// Decodes a data run list. Returns `None` if it is malformed.
pub(crate) fn decode_runs(b: &[u8]) -> Option<Vec<Run>> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;
    loop {
        let header = *b.get(pos)?;
        if header == 0 {
            return Some(runs);
        }
        let len_size = usize::from(header & 0x0F);
        let off_size = usize::from(header >> 4);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return None;
        }
        pos += 1;
        let len_bytes = b.get(pos..pos + len_size)?;
        let mut clusters: u64 = 0;
        for (i, byte) in len_bytes.iter().enumerate() {
            clusters |= u64::from(*byte) << (8 * i);
        }
        pos += len_size;
        let run_lcn = if off_size == 0 {
            None
        } else {
            let off_bytes = b.get(pos..pos + off_size)?;
            let mut delta: i64 = 0;
            for (i, byte) in off_bytes.iter().enumerate() {
                delta |= i64::from(*byte) << (8 * i);
            }
            // Sign-extend from the top byte.
            let shift = 64 - 8 * off_size as u32;
            if shift > 0 {
                delta = (delta << shift) >> shift;
            }
            pos += off_size;
            lcn = lcn.checked_add(delta)?;
            Some(u64::try_from(lcn).ok()?)
        };
        runs.push(Run {
            lcn: run_lcn,
            clusters,
        });
    }
}

/// Applies the update sequence fixups to a record in place. Returns false
/// if the record is torn (a block's check value does not match), in which
/// case it must not be trusted.
pub(crate) fn apply_fixups(rec: &mut [u8]) -> bool {
    let (Some(usa_ofs), Some(usa_count)) = (u16_at(rec, 4), u16_at(rec, 6)) else {
        return false;
    };
    let usa_ofs = usize::from(usa_ofs);
    let usa_count = usize::from(usa_count);
    if usa_count < 2 || (usa_count - 1) * FIXUP_BLOCK > rec.len() {
        return false;
    }
    let Some(check) = u16_at(rec, usa_ofs) else {
        return false;
    };
    for i in 1..usa_count {
        let Some(original) = u16_at(rec, usa_ofs + 2 * i) else {
            return false;
        };
        let end = i * FIXUP_BLOCK;
        if u16_at(rec, end - 2) != Some(check) {
            return false;
        }
        rec[end - 2..end].copy_from_slice(&original.to_le_bytes());
    }
    true
}

/// One `$FILE_NAME` of a record: a link from a parent directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileName {
    /// Parent directory file reference (record number + sequence).
    pub(crate) parent: u64,
    pub(crate) namespace: u8,
    pub(crate) name: Vec<u16>,
    /// The reparse tag recorded in the name's duplicated information.
    pub(crate) reparse_tag: u32,
}

/// Sizes of the unnamed `$DATA` stream (or one segment of it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DataSizes {
    pub(crate) apparent: u64,
    pub(crate) allocated: u64,
    /// Total allocated clusters in bytes for compressed or sparse streams.
    pub(crate) compressed: Option<u64>,
}

/// Everything Hoard needs from one MFT record. Attributes of a large file
/// can be spread over a base record and extension records; each is parsed
/// on its own and merged by the caller using `base`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Record {
    pub(crate) sequence: u16,
    pub(crate) in_use: bool,
    pub(crate) is_dir: bool,
    /// Record number of the base record if this is an extension record.
    pub(crate) base: Option<u64>,
    /// `$STANDARD_INFORMATION`: (file attributes, last write time ticks).
    pub(crate) standard: Option<(u32, u64)>,
    pub(crate) names: Vec<FileName>,
    /// Unnamed `$DATA` sizes, only from the segment starting at VCN 0 (the
    /// only one whose header carries the stream sizes).
    pub(crate) data: Option<DataSizes>,
    /// Data runs of the unnamed `$DATA` segment(s) in this record, with the
    /// segment's starting VCN. Only kept when `want_runs` is set (for `$MFT`).
    pub(crate) data_runs: Vec<(u64, Vec<Run>)>,
    /// Allocated size of the `$I30` directory index.
    pub(crate) index_allocated: Option<u64>,
    /// Tag from a resident `$REPARSE_POINT`.
    pub(crate) reparse_tag: Option<u32>,
    /// First cluster of a non-resident `$REPARSE_POINT` (long junction and
    /// symlink targets), where its tag must be read from.
    pub(crate) reparse_lcn: Option<u64>,
    /// Records named by a resident `$ATTRIBUTE_LIST` as holding segments of
    /// the unnamed `$DATA` stream. Only needed for `$MFT` itself, whose runs
    /// must be known before any other record can be read.
    pub(crate) data_segments: Vec<u64>,
}

/// Parses a record whose fixups have already been applied. Returns `None`
/// for anything that is not a well-formed `FILE` record.
pub(crate) fn parse_record(rec: &[u8], want_runs: bool) -> Option<Record> {
    if rec.get(0..4) != Some(b"FILE".as_slice()) {
        return None;
    }
    let flags = u16_at(rec, 0x16)?;
    let base_ref = u64_at(rec, 0x20)? & RECORD_NUMBER_MASK;
    let mut out = Record {
        sequence: u16_at(rec, 0x10)?,
        in_use: flags & RECORD_IN_USE != 0,
        is_dir: flags & RECORD_IS_DIRECTORY != 0,
        base: (base_ref != 0).then_some(base_ref),
        ..Record::default()
    };
    if !out.in_use {
        return Some(out);
    }
    let used = (u32_at(rec, 0x18)? as usize).min(rec.len());
    let mut off = usize::from(u16_at(rec, 0x14)?);
    while off + 8 <= used {
        let kind = u32_at(rec, off)?;
        if kind == ATTR_END {
            break;
        }
        let len = u32_at(rec, off + 4)? as usize;
        if len < 16 || off + len > used {
            return None;
        }
        let attr = &rec[off..off + len];
        parse_attribute(attr, kind, want_runs, &mut out)?;
        off += len;
    }
    Some(out)
}

fn parse_attribute(a: &[u8], kind: u32, want_runs: bool, out: &mut Record) -> Option<()> {
    let non_resident = *a.get(8)? != 0;
    let name_len = usize::from(*a.get(9)?);
    let flags = u16_at(a, 0x0C)?;
    let resident_value = || -> Option<&[u8]> {
        let vlen = u32_at(a, 0x10)? as usize;
        let voff = usize::from(u16_at(a, 0x14)?);
        a.get(voff..voff.checked_add(vlen)?)
    };
    match kind {
        ATTR_STANDARD_INFORMATION if !non_resident => {
            let v = resident_value()?;
            out.standard = Some((u32_at(v, 0x20)?, u64_at(v, 0x08)?));
        }
        ATTR_FILE_NAME if !non_resident => {
            let v = resident_value()?;
            let len = usize::from(*v.get(0x40)?);
            let raw = v.get(0x42..0x42 + 2 * len)?;
            out.names.push(FileName {
                parent: u64_at(v, 0)?,
                namespace: *v.get(0x41)?,
                name: raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect(),
                reparse_tag: u32_at(v, 0x3C)?,
            });
        }
        ATTR_DATA if name_len == 0 => {
            if non_resident {
                let start_vcn = u64_at(a, 0x10)?;
                if start_vcn == 0 {
                    let compressed = if flags & (ATTR_FLAG_COMPRESSED | ATTR_FLAG_SPARSE) != 0 {
                        Some(u64_at(a, 0x40)?)
                    } else {
                        None
                    };
                    out.data = Some(DataSizes {
                        allocated: u64_at(a, 0x28)?,
                        apparent: u64_at(a, 0x30)?,
                        compressed,
                    });
                }
                if want_runs {
                    let runs_off = usize::from(u16_at(a, 0x20)?);
                    out.data_runs
                        .push((start_vcn, decode_runs(a.get(runs_off..)?)?));
                }
            } else {
                let v = resident_value()?;
                let size = v.len() as u64;
                out.data = Some(DataSizes {
                    apparent: size,
                    // Resident data lives inside the record; Windows reports
                    // its allocation as the size rounded up to 8 bytes.
                    allocated: size.div_ceil(8) * 8,
                    compressed: None,
                });
            }
        }
        ATTR_INDEX_ALLOCATION if non_resident && is_i30(a, name_len) => {
            if u64_at(a, 0x10)? == 0 {
                out.index_allocated = Some(u64_at(a, 0x28)?);
            }
        }
        ATTR_ATTRIBUTE_LIST if !non_resident && want_runs => {
            let v = resident_value()?;
            let mut pos = 0usize;
            while pos + 0x1A <= v.len() {
                let entry_type = u32_at(v, pos)?;
                let entry_len = usize::from(u16_at(v, pos + 4)?);
                let entry_name_len = *v.get(pos + 6)?;
                if entry_len < 0x1A {
                    return None;
                }
                if entry_type == ATTR_DATA && entry_name_len == 0 {
                    let segment = u64_at(v, pos + 0x10)? & RECORD_NUMBER_MASK;
                    if !out.data_segments.contains(&segment) {
                        out.data_segments.push(segment);
                    }
                }
                pos += entry_len;
            }
        }
        ATTR_REPARSE_POINT if !non_resident => {
            out.reparse_tag = Some(u32_at(resident_value()?, 0)?);
        }
        ATTR_REPARSE_POINT => {
            let runs_off = usize::from(u16_at(a, 0x20)?);
            let runs = decode_runs(a.get(runs_off..)?)?;
            out.reparse_lcn = runs.first().and_then(|r| r.lcn);
        }
        _ => {}
    }
    Some(())
}

/// Whether a named attribute is `$I30` (the file name index of a directory).
fn is_i30(a: &[u8], name_len: usize) -> bool {
    const I30: [u16; 4] = [b'$' as u16, b'I' as u16, b'3' as u16, b'0' as u16];
    if name_len != I30.len() {
        return false;
    }
    let Some(off) = u16_at(a, 0x0A).map(usize::from) else {
        return false;
    };
    a.get(off..off + 8).is_some_and(|raw| {
        raw.as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .eq(I30.iter().copied())
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds MFT records byte by byte for tests.
    pub(crate) struct RecordBuilder {
        attrs: Vec<u8>,
        flags: u16,
        sequence: u16,
        base: u64,
    }

    pub(crate) const RECORD_SIZE: usize = 1024;

    impl RecordBuilder {
        pub(crate) fn new(sequence: u16, is_dir: bool) -> Self {
            RecordBuilder {
                attrs: Vec::new(),
                flags: RECORD_IN_USE | if is_dir { RECORD_IS_DIRECTORY } else { 0 },
                sequence,
                base: 0,
            }
        }

        pub(crate) fn unused(mut self) -> Self {
            self.flags &= !RECORD_IN_USE;
            self
        }

        pub(crate) fn extension_of(mut self, base: u64) -> Self {
            self.base = base;
            self
        }

        fn push_attr(
            &mut self,
            kind: u32,
            non_resident: bool,
            name: &str,
            flags: u16,
            body: &[u8],
        ) {
            let name16: Vec<u16> = name.encode_utf16().collect();
            let header_len = if non_resident { 0x48 } else { 0x18 };
            let name_off = header_len;
            let body_off = (name_off + name16.len() * 2).div_ceil(8) * 8;
            let len = (body_off + body.len()).div_ceil(8) * 8;
            let mut a = vec![0u8; len];
            a[0..4].copy_from_slice(&kind.to_le_bytes());
            a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
            a[8] = u8::from(non_resident);
            a[9] = name16.len() as u8;
            a[0x0A..0x0C].copy_from_slice(&(name_off as u16).to_le_bytes());
            a[0x0C..0x0E].copy_from_slice(&flags.to_le_bytes());
            for (i, c) in name16.iter().enumerate() {
                a[name_off + 2 * i..name_off + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
            }
            if non_resident {
                // `body` is the non-resident header tail from 0x10 on, then runs.
                a[0x10..0x10 + body.len().min(0x30)].copy_from_slice(&body[..body.len().min(0x30)]);
                let runs = &body[body.len().min(0x30)..];
                let runs_off = body_off.max(0x48);
                let len = (runs_off + runs.len() + 1).div_ceil(8) * 8;
                a.resize(len, 0);
                a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
                a[0x20..0x22].copy_from_slice(&(runs_off as u16).to_le_bytes());
                a[runs_off..runs_off + runs.len()].copy_from_slice(runs);
            } else {
                a[0x10..0x14].copy_from_slice(&(body.len() as u32).to_le_bytes());
                a[0x14..0x16].copy_from_slice(&(body_off as u16).to_le_bytes());
                a[body_off..body_off + body.len()].copy_from_slice(body);
            }
            self.attrs.extend_from_slice(&a);
        }

        pub(crate) fn standard(mut self, attrs: u32, mtime_ticks: u64) -> Self {
            let mut v = vec![0u8; 0x48];
            v[0x08..0x10].copy_from_slice(&mtime_ticks.to_le_bytes());
            v[0x20..0x24].copy_from_slice(&attrs.to_le_bytes());
            self.push_attr(ATTR_STANDARD_INFORMATION, false, "", 0, &v);
            self
        }

        pub(crate) fn name(mut self, parent_ref: u64, namespace: u8, name: &str) -> Self {
            let n: Vec<u16> = name.encode_utf16().collect();
            let mut v = vec![0u8; 0x42 + 2 * n.len()];
            v[0..8].copy_from_slice(&parent_ref.to_le_bytes());
            v[0x40] = n.len() as u8;
            v[0x41] = namespace;
            for (i, c) in n.iter().enumerate() {
                v[0x42 + 2 * i..0x44 + 2 * i].copy_from_slice(&c.to_le_bytes());
            }
            self.push_attr(ATTR_FILE_NAME, false, "", 0, &v);
            self
        }

        pub(crate) fn resident_data(mut self, stream: &str, len: usize) -> Self {
            self.push_attr(ATTR_DATA, false, stream, 0, &vec![0xAB; len]);
            self
        }

        /// Non-resident attribute with the given sizes and run list bytes.
        #[allow(clippy::too_many_arguments)]
        pub(crate) fn non_resident(
            mut self,
            kind: u32,
            stream: &str,
            flags: u16,
            start_vcn: u64,
            allocated: u64,
            apparent: u64,
            compressed: u64,
            runs: &[u8],
        ) -> Self {
            let mut body = vec![0u8; 0x30];
            body[0x00..0x08].copy_from_slice(&start_vcn.to_le_bytes()); // 0x10
            body[0x18..0x20].copy_from_slice(&allocated.to_le_bytes()); // 0x28
            body[0x20..0x28].copy_from_slice(&apparent.to_le_bytes()); // 0x30
            body[0x28..0x30].copy_from_slice(&apparent.to_le_bytes()); // 0x38
            body.extend_from_slice(runs);
            self.push_attr(kind, true, stream, flags, &body);
            if compressed != 0 {
                // Total allocated lives at 0x40 for compressed/sparse streams.
                let start = self.attrs.len() - self.last_len();
                let at = start + 0x40;
                self.attrs[at..at + 8].copy_from_slice(&compressed.to_le_bytes());
            }
            self
        }

        fn last_len(&self) -> usize {
            // Walk the attribute chain to find the last attribute's length.
            let mut off = 0;
            let mut last = 0;
            while off < self.attrs.len() {
                last =
                    u32::from_le_bytes(self.attrs[off + 4..off + 8].try_into().unwrap()) as usize;
                off += last;
            }
            last
        }

        pub(crate) fn data(self, allocated: u64, apparent: u64, runs: &[u8]) -> Self {
            self.non_resident(ATTR_DATA, "", 0, 0, allocated, apparent, 0, runs)
        }

        pub(crate) fn index_allocation(self, allocated: u64) -> Self {
            self.non_resident(
                ATTR_INDEX_ALLOCATION,
                "$I30",
                0,
                0,
                allocated,
                allocated,
                0,
                &[0x11, 0x01, 0x10],
            )
        }

        /// A resident attribute list naming `$DATA` segments in `records`.
        pub(crate) fn attribute_list(mut self, records: &[u64]) -> Self {
            let mut v = Vec::new();
            for r in records {
                let mut e = vec![0u8; 0x20];
                e[0..4].copy_from_slice(&ATTR_DATA.to_le_bytes());
                e[4..6].copy_from_slice(&0x20u16.to_le_bytes());
                e[0x10..0x18].copy_from_slice(&r.to_le_bytes());
                v.extend_from_slice(&e);
            }
            self.push_attr(ATTR_ATTRIBUTE_LIST, false, "", 0, &v);
            self
        }

        /// A non-resident `$REPARSE_POINT` whose data starts at `lcn`.
        pub(crate) fn reparse_at(self, lcn_run: &[u8]) -> Self {
            self.non_resident(ATTR_REPARSE_POINT, "", 0, 0, 4096, 900, 0, lcn_run)
        }

        pub(crate) fn reparse(mut self, tag: u32) -> Self {
            let mut v = vec![0u8; 8];
            v[0..4].copy_from_slice(&tag.to_le_bytes());
            self.push_attr(ATTR_REPARSE_POINT, false, "", 0, &v);
            self
        }

        /// Serializes with a valid update sequence array (check value 0x0007).
        pub(crate) fn build(&self) -> Vec<u8> {
            let mut r = vec![0u8; RECORD_SIZE];
            r[0..4].copy_from_slice(b"FILE");
            let usa_ofs = 0x30usize;
            let usa_count = RECORD_SIZE / FIXUP_BLOCK + 1;
            r[4..6].copy_from_slice(&(usa_ofs as u16).to_le_bytes());
            r[6..8].copy_from_slice(&(usa_count as u16).to_le_bytes());
            r[0x10..0x12].copy_from_slice(&self.sequence.to_le_bytes());
            let first = (usa_ofs + 2 * usa_count).div_ceil(8) * 8;
            r[0x14..0x16].copy_from_slice(&(first as u16).to_le_bytes());
            r[0x16..0x18].copy_from_slice(&self.flags.to_le_bytes());
            let used = first + self.attrs.len() + 8;
            assert!(used <= RECORD_SIZE, "record too big");
            r[0x18..0x1C].copy_from_slice(&(used as u32).to_le_bytes());
            r[0x1C..0x20].copy_from_slice(&(RECORD_SIZE as u32).to_le_bytes());
            r[0x20..0x28].copy_from_slice(&self.base.to_le_bytes());
            r[first..first + self.attrs.len()].copy_from_slice(&self.attrs);
            r[first + self.attrs.len()..first + self.attrs.len() + 4]
                .copy_from_slice(&ATTR_END.to_le_bytes());
            // Fixups: stash each block's last two bytes, write the check value.
            let check: u16 = 0x0007;
            r[usa_ofs..usa_ofs + 2].copy_from_slice(&check.to_le_bytes());
            for i in 1..usa_count {
                let end = i * FIXUP_BLOCK;
                let orig = [r[end - 2], r[end - 1]];
                r[usa_ofs + 2 * i..usa_ofs + 2 * i + 2].copy_from_slice(&orig);
                r[end - 2..end].copy_from_slice(&check.to_le_bytes());
            }
            r
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Record {
        let mut b = bytes.to_vec();
        assert!(apply_fixups(&mut b), "fixups");
        parse_record(&b, true).expect("parse")
    }

    #[test]
    fn data_runs_decode_with_signed_offsets_and_sparse_runs() {
        // 0x21: 1-byte length, 2-byte offset. 0x01: sparse run. 0x11 negative.
        let raw = [
            0x21, 0x10, 0x00, 0x01, // 16 clusters at LCN 256
            0x01, 0x08, // 8 sparse clusters
            0x11, 0x04, 0xF0, // 4 clusters at 256 - 16 = 240
            0x00,
        ];
        assert_eq!(
            decode_runs(&raw).expect("runs"),
            vec![
                Run {
                    lcn: Some(256),
                    clusters: 16
                },
                Run {
                    lcn: None,
                    clusters: 8
                },
                Run {
                    lcn: Some(240),
                    clusters: 4
                },
            ]
        );
        assert!(decode_runs(&[0x21, 0x10]).is_none(), "truncated");
        assert!(decode_runs(&[0x11, 0x01, 0x80]).is_none(), "negative LCN");
    }

    #[test]
    fn fixups_restore_bytes_and_reject_torn_records() {
        let built = RecordBuilder::new(3, false).name(5, 1, "a.txt").build();
        let mut ok = built.clone();
        assert!(apply_fixups(&mut ok));
        // A torn write leaves an old check value in the second block.
        let mut torn = built.clone();
        torn[2 * FIXUP_BLOCK - 2] = 0x99;
        assert!(!apply_fixups(&mut torn));
        // Bogus array sizes are rejected, not indexed.
        let mut bad = built;
        bad[6..8].copy_from_slice(&200u16.to_le_bytes());
        assert!(!apply_fixups(&mut bad));
    }

    #[test]
    fn parses_a_file_record() {
        let r = parse(
            &RecordBuilder::new(9, false)
                .standard(0x20, 133_000_000_000_000_000)
                .name(5 | (5 << 48), 2, "LONGFI~1.TXT")
                .name(5 | (5 << 48), 1, "long file name.txt")
                .data(8192, 5000, &[0x11, 0x02, 0x40, 0x00])
                .resident_data("Zone.Identifier", 26)
                .build(),
        );
        assert!(r.in_use && !r.is_dir);
        assert_eq!(r.sequence, 9);
        assert_eq!(r.standard, Some((0x20, 133_000_000_000_000_000)));
        assert_eq!(r.names.len(), 2);
        assert_eq!(
            String::from_utf16_lossy(&r.names[1].name),
            "long file name.txt"
        );
        assert_eq!(r.names[1].parent & RECORD_NUMBER_MASK, 5);
        // The named stream (ADS) does not count; the unnamed one does.
        assert_eq!(
            r.data,
            Some(DataSizes {
                apparent: 5000,
                allocated: 8192,
                compressed: None
            })
        );
        assert_eq!(
            r.data_runs,
            vec![(
                0,
                vec![Run {
                    lcn: Some(64),
                    clusters: 2
                }]
            )]
        );
    }

    #[test]
    fn resident_data_allocation_rounds_to_eight() {
        let r = parse(&RecordBuilder::new(1, false).resident_data("", 13).build());
        assert_eq!(
            r.data,
            Some(DataSizes {
                apparent: 13,
                allocated: 16,
                compressed: None
            })
        );
    }

    #[test]
    fn compressed_streams_report_total_allocated() {
        let r = parse(
            &RecordBuilder::new(1, false)
                .non_resident(
                    ATTR_DATA,
                    "",
                    ATTR_FLAG_COMPRESSED,
                    0,
                    65536,
                    60000,
                    12288,
                    &[0x11, 0x10, 0x20],
                )
                .build(),
        );
        assert_eq!(
            r.data,
            Some(DataSizes {
                apparent: 60000,
                allocated: 65536,
                compressed: Some(12288)
            })
        );
    }

    #[test]
    fn later_data_segments_do_not_override_sizes() {
        let r = parse(
            &RecordBuilder::new(1, false)
                .extension_of(77)
                .non_resident(ATTR_DATA, "", 0, 500, 0, 0, 0, &[0x11, 0x10, 0x20])
                .build(),
        );
        assert_eq!(r.base, Some(77));
        assert_eq!(r.data, None);
        assert_eq!(r.data_runs.len(), 1);
        assert_eq!(r.data_runs[0].0, 500);
    }

    #[test]
    fn directories_index_and_reparse_points() {
        let d = parse(
            &RecordBuilder::new(2, true)
                .standard(0x10, 1)
                .name(5, 1, "dir")
                .index_allocation(16384)
                .build(),
        );
        assert!(d.is_dir);
        assert_eq!(d.index_allocated, Some(16384));

        let j = parse(
            &RecordBuilder::new(2, true)
                .standard(FILE_ATTRIBUTE_REPARSE_POINT, 1)
                .name(5, 1, "junction")
                .reparse(0xA000_0003)
                .build(),
        );
        assert_eq!(j.reparse_tag, Some(0xA000_0003));
    }

    #[test]
    fn unused_and_garbage_records() {
        let r = parse(
            &RecordBuilder::new(4, false)
                .unused()
                .name(5, 1, "gone")
                .build(),
        );
        assert!(!r.in_use);
        assert!(r.names.is_empty(), "attributes of free records are ignored");
        assert!(parse_record(&[0u8; 1024], false).is_none());
        let mut b = RecordBuilder::new(1, false).name(5, 1, "x").build();
        assert!(apply_fixups(&mut b));
        // An attribute length running past the used size is rejected.
        let first = usize::from(u16::from_le_bytes([b[0x14], b[0x15]]));
        b[first + 4..first + 8].copy_from_slice(&5000u32.to_le_bytes());
        assert!(parse_record(&b, false).is_none());
    }

    #[test]
    fn boot_sector_geometry() {
        let mut b = vec![0u8; 512];
        b[3..11].copy_from_slice(b"NTFS    ");
        b[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        b[0x0D] = 8;
        b[0x30..0x38].copy_from_slice(&786_432u64.to_le_bytes());
        b[0x40] = 0xF6; // -10: 1024-byte records
        b[0x48..0x50].copy_from_slice(&0x1234_5678_9ABC_DEF0u64.to_le_bytes());
        let bs = BootSector::parse(&b).expect("boot");
        assert_eq!(bs.cluster_size, 4096);
        assert_eq!(bs.record_size, 1024);
        assert_eq!(bs.mft_lcn, 786_432);
        assert_eq!(bs.serial, 0x1234_5678_9ABC_DEF0);
        b[0x0D] = 0xF0; // 2^16 sectors per cluster: 32 MiB, too large
        assert!(BootSector::parse(&b).is_err());
        b[3] = b'X';
        assert!(BootSector::parse(&b).is_err());
    }
}
