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

## Known bottleneck

On Windows every entry is opened with `CreateFileW` to read its file index,
volume serial and link count (needed for hardlink dedup and, in M2, for
cleanup revalidation). That costs about 100 us per entry single-threaded.

Candidates, in order:

1. Enumerate with `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` on
   the directory handle. This returns file ID, sizes, attributes, reparse tag
   and mtime for a whole directory in one call, with no per-file open. It does
   not return the link count, so hardlink dedup would need either a per-volume
   file ID set (memory cost) or a targeted open only where hardlinks are
   likely.
2. NTFS MFT fast path when elevated (PRD S10, P2).
