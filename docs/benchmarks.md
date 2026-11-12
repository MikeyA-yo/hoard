# Benchmarks

Run with `cargo bench -p hoard-core`. Note any regression over 10% against
the latest baseline below, with the commit that caused it.

## Baseline (M0, 8 Nov 2026)

**Machine:** Intel Core i7-8650U (4 cores / 8 threads, 1.9 GHz), 16 GB RAM,
512 GB M.2 SATA SSD, Windows 11 Pro 23H2, Rust 1.89 (windows-gnu),
Windows Defender real-time protection on.

### `scan/100k_files` (criterion)

Generated tree: 50 x 20 directories, 100 files each (100,000 files, 1,050
directories), sizes 0 to 8 KiB. Warm metadata cache, default options
(16 threads).

| Metric | Value |
|--------|-------|
| Time per scan | 1.71 s (range 1.67 to 1.77 s) |
| Throughput | 58.5k files/s |
| Extrapolated 1M files | about 17 s (PRD target: under 15 s on NVMe) |

### Real trees (release CLI)

| Tree | Entries | Cold | Warm | vs `du` (allocated) |
|------|---------|------|------|---------------------|
| `~/.cargo` | 28.5k | 6.4 s | 0.54 s | 831.4 MB vs 834.6 MB, 0.4% low |
| `~` (home) | 3.31M | 687 s (*) | not measured | not compared |

(*) Cold cache, with a `cargo clippy` cross-compile running at the same
time. Treat it as a worst case, not a baseline.

Apparent size on `~/.cargo` matched `du --apparent-size` to within its 1 KiB
rounding (781,447,872 B vs 763,133 KiB).

### Thread scaling (`~/.cargo`, warm)

| Threads | Time |
|---------|------|
| 1 | 2.93 s |
| 4 | 0.94 s |
| 16 (default, 2x CPUs) | 0.54 to 0.78 s |
| 48 | 0.77 s |

## Windows per-directory stat (11 Nov 2026)

Same machine, Rust 1.99 (windows-gnu). Entries are now queried with
`NtQueryInformationByName(FileStatInformation)` relative to one open handle
per directory (`platform::DirStat`), instead of `CreateFileW` +
`GetFileInformationByHandle(Ex)` + `CloseHandle` per entry. One call returns
file ID, link count, sizes, attributes and reparse tag. A directory's own
index allocation is read from the handle the scanner opens to list it, so
directories need no extra open.

### `scan/100k_files` (criterion)

| Metric | Before | After |
|--------|--------|-------|
| Time per scan | 1.71 s | 0.79 s (range 0.77 to 0.82 s) |
| Throughput | 58.5k files/s | 127k files/s |

### Real trees (release CLI, warm, old and new alternated)

| Tree | Entries | Before | After | Speedup |
|------|---------|--------|-------|---------|
| `~/.cargo` | 44k | 2.2 s | 1.4 s | 1.6x |
| `~/.gemini` (many small files) | 48k | 2.8 s | 0.65 s | 4.2x |
| `~/AppData` | 1.05M | 51 s | 22 s | 2.3x |
| `~` (home) | 3.48M | 372 s | 342 s | 1.1x |

Totals are byte-identical to the previous scanner on `~/.cargo`, `~/.rustup`,
`~/go`, `~/Documents` and the pnpm store (an earlier draft undercounted large
directories, see `scan_totals_match_per_entry_stat`). A probe over 136k
entries in pnpm trees (67k hardlinked) found no metadata differences at all.

The whole home folder barely improves: its 3.5M file records (about 3.5 GB of
MFT) do not fit in the file cache next to everything else on this 16 GB
machine, so every pass is mostly disk reads, which the per-entry query still
needs. Single-threaded micro-benchmark on `~/.cargo/registry` (44k entries):
per-entry open 13.0 s, per-directory query 3.2 s, directory enumeration alone
1.9 s.

## Remaining bottleneck

1. Disk reads of file records on large, cold trees. Directory enumeration
   (`FileIdExtdDirectoryInfo`) avoids them but has no link count and can
   carry stale sizes for hardlinked files, so it is not used.
2. Each directory is opened twice (jwalk's listing and `DirStat`). A custom
   walker that lists through the same handle would remove one open per
   directory.
3. NTFS MFT fast path when elevated (PRD S10, P2): done, see below.

## NTFS MFT fast path (12 Nov 2026)

Elevated scans of a drive root, or the home folder and its parents, read the
Master File Table from the raw volume (`scan/mft`) instead of walking. Reader
threads fetch chunks of the table, rayon parses each chunk's records in
parallel, and the tree is built from the parent links.

Machine: C: is a 512 GB SATA M.2 SSD, about 92% full; the MFT holds 3.78M
records (about 3.7 GB).

| Scan of `C:\` | Time |
|---------------|------|
| Walk of `~` alone (3.48M entries, see above) | 342 s |
| MFT, whole drive (3.43M files, 428k folders) | 39 to 100 s |

The MFT time is almost all disk reads and varies with the drive's state: the
same settings took 39 s on 11 Nov and 64 s and 74 s on 12 Nov. Parsing takes
2 to 6 s, building the tree 3 to 7 s.

### Reader settings (`HOARD_MFT_READERS` x `HOARD_MFT_CHUNK_MB`, one session)

| Readers x MB | Table read | Total |
|--------------|------------|-------|
| 1 x 16 | 65.1 s | 74.2 s |
| 4 x 4 | 90.8 s | 99.8 s |
| 8 x 2 (default) | 53.5 s | 58.3 s |
| 4 x 16 | 55.0 s | 61.7 s |
| 16 x 1 | 66.1 s | 72.2 s |

On this SATA drive, throughput stays around 55 to 70 MB/s however many reads
are in flight; the spread is run-to-run noise. 8 x 2 was fastest, keeps about
50 MB of buffers, and favours NVMe drives, which gain from parallel reads.

Accuracy (`hoard compare-mft`): `~/.cargo` and the pnpm store match the walk
exactly. On the whole home folder totals agree within 0.003%; the remaining
differences are files that changed between the two scans, plus sparse and
compressed files where the MFT reports the true allocation.
