//! Scan benchmark over a generated 100k-file tree.
//!
//! Layout: 50 top-level dirs x 20 subdirs x 100 files = 100,000 files plus
//! 1,050 directories, sizes 0..8 KiB. The tree is created once in a temp dir
//! (outside the timed region) and the scan runs warm.
//!
//! Run: `cargo bench -p hoard-core`. Record results in `docs/benchmarks.md`.

use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use hoard_core::{scan, ScanOptions, SizeMode};

const TOP: usize = 50;
const SUB: usize = 20;
const FILES: usize = 100;
const TOTAL_FILES: usize = TOP * SUB * FILES;

fn generate(root: &Path) {
    let buf = vec![0x5Au8; 8192];
    let mut n: usize = 0;
    for t in 0..TOP {
        for s in 0..SUB {
            let dir = root.join(format!("top{t:02}")).join(format!("sub{s:02}"));
            fs::create_dir_all(&dir).expect("create dir");
            for f in 0..FILES {
                // Deterministic pseudo-random size in 0..8192.
                n = n
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let size = (n >> 33) % 8192;
                fs::write(dir.join(format!("file{f:03}.dat")), &buf[..size]).expect("write file");
            }
        }
    }
}

fn bench_scan(c: &mut Criterion) {
    let dir = tempfile::Builder::new()
        .prefix("hoard-bench-")
        .tempdir()
        .expect("temp dir");
    eprintln!(
        "generating {TOTAL_FILES} files in {} ...",
        dir.path().display()
    );
    generate(dir.path());

    let run = || {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let tree = scan(
            dir.path(),
            ScanOptions::default(),
            tx,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("scan");
        assert_eq!(
            tree.get(tree.root()).map(|n| n.file_count as usize),
            Some(TOTAL_FILES)
        );
        black_box(tree.total(SizeMode::Allocated))
    };
    // Warm the OS metadata cache before measuring.
    run();

    let mut group = c.benchmark_group("scan");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));
    group.throughput(Throughput::Elements(TOTAL_FILES as u64));
    group.bench_function("100k_files", |b| b.iter(run));
    group.finish();
}

criterion_group!(benches, bench_scan);
criterion_main!(benches);
