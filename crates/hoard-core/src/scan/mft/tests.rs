//! MFT fast path tests on synthetic in-memory volumes.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use super::record::tests::{RecordBuilder, RECORD_SIZE};
use super::record::{FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_REPARSE_POINT};
use super::*;
use crate::tree::{NodeId, ScanSource};

const CLUSTER: usize = 4096;
const SERIAL: u64 = 0x0000_0000_BEEF_CAFE;
/// 2023-11-14 22:13:20 UTC in FILETIME ticks (unix 1_700_000_000).
const T: u64 = (1_700_000_000 + 11_644_473_600) * 10_000_000;

fn fref(record: u64, seq: u16) -> u64 {
    (u64::from(seq) << 48) | record
}

/// Encodes one data run.
fn run(lcn_delta: i64, clusters: u64) -> Vec<u8> {
    let len: Vec<u8> = clusters.to_le_bytes().into_iter().take(8).collect();
    let len_size = (8 - clusters.leading_zeros() as usize / 8).max(1);
    let off: Vec<u8> = lcn_delta.to_le_bytes().to_vec();
    // Smallest signed width that round-trips.
    let mut off_size = 8;
    while off_size > 1 {
        let shift = 64 - 8 * (off_size - 1) as u32;
        if (lcn_delta << shift) >> shift == lcn_delta {
            off_size -= 1;
        } else {
            break;
        }
    }
    let mut out = vec![(off_size as u8) << 4 | len_size as u8];
    out.extend_from_slice(&len[..len_size]);
    out.extend_from_slice(&off[..off_size]);
    out
}

/// A synthetic NTFS volume: boot sector, then an MFT that may be split into
/// two extents (to exercise `$MFT` extension records).
struct Image {
    records: Vec<Vec<u8>>,
    /// MFT extents: (LCN, clusters).
    extents: Vec<(u64, u64)>,
    /// Raw data clusters outside the MFT: (LCN, bytes).
    blobs: Vec<(u64, Vec<u8>)>,
}

impl Image {
    fn bytes(&self) -> Vec<u8> {
        let end = self
            .extents
            .iter()
            .map(|(l, c)| (l + c) as usize * CLUSTER)
            .max()
            .unwrap();
        let mut v = vec![0u8; end];
        v[3..11].copy_from_slice(b"NTFS    ");
        v[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        v[0x0D] = (CLUSTER / 512) as u8;
        v[0x30..0x38].copy_from_slice(&self.extents[0].0.to_le_bytes());
        v[0x40] = 0xF6; // 1024-byte records
        v[0x48..0x50].copy_from_slice(&SERIAL.to_le_bytes());
        let per_cluster = CLUSTER / RECORD_SIZE;
        for (i, rec) in self.records.iter().enumerate() {
            let mut cluster = i / per_cluster;
            let mut placed = None;
            for (lcn, clusters) in &self.extents {
                if (cluster as u64) < *clusters {
                    placed = Some(*lcn as usize + cluster);
                    break;
                }
                cluster -= *clusters as usize;
            }
            let at = placed.unwrap() * CLUSTER + (i % per_cluster) * RECORD_SIZE;
            v[at..at + RECORD_SIZE].copy_from_slice(rec);
        }
        for (lcn, data) in &self.blobs {
            let at = *lcn as usize * CLUSTER;
            v[at..at + data.len()].copy_from_slice(data);
        }
        v
    }
}

/// The standard test volume. Records:
/// 0 $MFT, 2 $LogFile, 5 root, 11 $Extend (+24 $UsnJrnl inside it),
/// 30 docs/, 31 docs/a.txt (+ DOS alias), 32 small.txt (resident),
/// 33 hard link (link1.bin in root, docs/link2.bin), 34 junction "jn" with a
/// bogus child 35, 36 big.iso whose data is in extension record 37,
/// 38 a deleted file, 39 a stale link into a reused docs record,
/// 40 torn, 41 compressed.txt.
fn volume() -> Image {
    let n = 48usize;
    let mft_clusters = (n * RECORD_SIZE).div_ceil(CLUSTER) as u64;
    let mut records = vec![vec![0u8; RECORD_SIZE]; n];
    let mut put = |i: usize, b: RecordBuilder| records[i] = b.build();
    let root = fref(5, 5);
    let docs = fref(30, 1);
    put(
        0,
        RecordBuilder::new(1, false)
            .standard(0x06, T)
            .name(root, 3, "$MFT")
            .data(
                mft_clusters * CLUSTER as u64,
                (n * RECORD_SIZE) as u64,
                &[run(4, mft_clusters), vec![0]].concat(),
            ),
    );
    put(
        2,
        RecordBuilder::new(2, false)
            .standard(0x06, T)
            .name(root, 3, "$LogFile")
            .data(65536, 65536, &[0]),
    );
    put(
        5,
        RecordBuilder::new(5, true)
            .standard(0x16, T)
            .name(root, 3, "."),
    );
    put(
        11,
        RecordBuilder::new(11, true)
            .standard(0x06, T)
            .name(root, 3, "$Extend"),
    );
    put(
        24,
        RecordBuilder::new(1, false)
            .standard(0x06, T)
            .name(fref(11, 11), 0, "$UsnJrnl")
            .data(1 << 20, 1 << 20, &[0]),
    );
    put(
        30,
        RecordBuilder::new(1, true)
            .standard(0x10, T)
            .name(root, 3, "docs")
            .index_allocation(8192),
    );
    put(
        31,
        RecordBuilder::new(3, false)
            .standard(0x20, T)
            .name(docs, 2, "ALONGN~1.TXT")
            .name(docs, 1, "a long name.txt")
            .data(12288, 10000, &[0]),
    );
    put(
        32,
        RecordBuilder::new(1, false)
            .standard(0x20, T)
            .name(root, 3, "small.txt")
            .resident_data("", 13),
    );
    put(
        33,
        RecordBuilder::new(1, false)
            .standard(0x20, T)
            .name(root, 1, "link1.bin")
            .name(docs, 1, "link2.bin")
            .data(40960, 40000, &[0]),
    );
    put(
        34,
        RecordBuilder::new(1, true)
            .standard(0x10 | FILE_ATTRIBUTE_REPARSE_POINT, T)
            .name(root, 3, "jn")
            .reparse(0xA000_0003),
    );
    put(
        35,
        RecordBuilder::new(1, false)
            .standard(0x20, T)
            .name(fref(34, 1), 3, "inside.bin")
            .data(4096, 4096, &[0]),
    );
    put(
        36,
        RecordBuilder::new(7, false)
            .standard(0x20, T)
            .name(root, 3, "big.iso"),
    );
    put(
        37,
        RecordBuilder::new(1, false)
            .extension_of(36)
            .data(1 << 30, (1 << 30) - 5, &[0]),
    );
    put(
        38,
        RecordBuilder::new(2, false)
            .unused()
            .name(root, 3, "deleted.bin"),
    );
    put(
        39,
        RecordBuilder::new(1, false)
            .standard(0x20, T)
            .name(fref(30, 99), 3, "stale.bin")
            .data(4096, 4096, &[0]),
    );
    let mut torn = RecordBuilder::new(1, false)
        .standard(0x20, T)
        .name(root, 3, "torn.bin")
        .build();
    torn[1022] ^= 0xFF;
    records[40] = torn;
    records[41] = RecordBuilder::new(1, false)
        .standard(0x20 | FILE_ATTRIBUTE_COMPRESSED, T)
        .name(root, 3, "compressed.txt")
        .non_resident(0x80, "", 0x0001, 0, 65536, 60000, 12288, &[0])
        .build();
    // 42: a junction with a long target, so its $REPARSE_POINT data lives in
    // cluster 2; 43 is a bogus child that must not be listed.
    let reparse_dir = 0x10 | FILE_ATTRIBUTE_REPARSE_POINT;
    records[42] = RecordBuilder::new(1, true)
        .standard(reparse_dir, T)
        .name(root, 3, "longjn")
        .reparse_at(&[run(2, 1), vec![0]].concat())
        .build();
    records[43] = RecordBuilder::new(1, false)
        .standard(0x20, T)
        .name(fref(42, 1), 3, "through.bin")
        .data(4096, 4096, &[0])
        .build();
    // 44: a cloud-synced folder (reparse point, not a link; data in cluster
    // 3) with a real child 45.
    records[44] = RecordBuilder::new(1, true)
        .standard(reparse_dir, T)
        .name(root, 3, "cloud")
        .reparse_at(&[run(3, 1), vec![0]].concat())
        .build();
    records[45] = RecordBuilder::new(1, false)
        .standard(0x20, T)
        .name(fref(44, 1), 3, "synced.txt")
        .data(4096, 100, &[0])
        .build();
    // 46: a reparse directory whose tag cannot be found at all.
    records[46] = RecordBuilder::new(1, true)
        .standard(reparse_dir, T)
        .name(root, 3, "mystery")
        .build();
    Image {
        records,
        extents: vec![(4, mft_clusters)],
        blobs: vec![
            (2, 0xA000_0003u32.to_le_bytes().to_vec()),
            (3, 0x9000_701Au32.to_le_bytes().to_vec()),
        ],
    }
}

fn root_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\")
    } else {
        PathBuf::from("/vol")
    }
}

fn dir_meta(record: u64, seq: u16) -> EntryMeta {
    EntryMeta {
        kind: NodeKind::Dir,
        dev: SERIAL,
        ino: fref(record, seq),
        nlink: 1,
        ..EntryMeta::default()
    }
}

fn run_scan(img: &[u8], root: &Path, meta: EntryMeta, opts: ScanOptions) -> Result<Tree, MftError> {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let reader: &[u8] = img;
    scan_volume(
        &reader,
        root,
        &meta,
        &opts,
        SizeMode::Allocated,
        tx,
        &AtomicBool::new(false),
        Instant::now(),
    )
}

fn child(tree: &Tree, parent: NodeId, name: &str) -> Option<NodeId> {
    tree.get(parent)?.children.iter().copied().find(|c| {
        tree.get(*c)
            .is_some_and(|n| n.name.to_string_lossy() == name)
    })
}

fn names(tree: &Tree, id: NodeId) -> Vec<String> {
    let mut v: Vec<String> = tree
        .get(id)
        .map(|n| {
            n.children
                .iter()
                .map(|c| tree.get(*c).unwrap().name.to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn builds_the_volume_tree() {
    let img = volume().bytes();
    let tree = run_scan(&img, &root_path(), dir_meta(5, 5), ScanOptions::default()).expect("scan");
    assert_eq!(tree.source(), ScanSource::Mft);
    assert!(tree.is_complete());
    let root = tree.root();
    // Metadata files, deleted records, stale links and the torn record are
    // absent; the DOS alias does not create a second entry.
    assert_eq!(
        names(&tree, root),
        [
            "big.iso",
            "cloud",
            "compressed.txt",
            "docs",
            "jn",
            "link1.bin",
            "longjn",
            "mystery",
            "small.txt"
        ]
    );
    let docs = child(&tree, root, "docs").unwrap();
    assert_eq!(names(&tree, docs), ["a long name.txt", "link2.bin"]);

    let a = tree
        .get(child(&tree, docs, "a long name.txt").unwrap())
        .unwrap();
    assert_eq!(
        (a.kind, a.size_apparent, a.size_allocated),
        (NodeKind::File, 10000, 12288)
    );
    assert_eq!(a.mtime, 1_700_000_000);
    assert_eq!((a.dev, a.ino), (SERIAL, fref(31, 3)));

    let small = tree.get(child(&tree, root, "small.txt").unwrap()).unwrap();
    assert_eq!((small.size_apparent, small.size_allocated), (13, 16));

    // Data from the extension record is merged into its base.
    let big = tree.get(child(&tree, root, "big.iso").unwrap()).unwrap();
    assert_eq!((big.size_apparent, big.ino), ((1 << 30) - 5, fref(36, 7)));

    // Compressed files count their compressed allocation.
    let c = tree
        .get(child(&tree, root, "compressed.txt").unwrap())
        .unwrap();
    assert_eq!((c.size_apparent, c.size_allocated), (60000, 12288));

    // Junctions are links: not descended, no size.
    let jn = tree.get(child(&tree, root, "jn").unwrap()).unwrap();
    assert_eq!(jn.kind, NodeKind::Symlink);
    assert!(jn.children.is_empty());

    // A junction whose reparse data is stored outside the record is still
    // a link; a cloud folder (reparse point, not a link) is descended; a
    // reparse folder of unknown kind is treated as a link.
    for (name, kind) in [
        ("longjn", NodeKind::Symlink),
        ("cloud", NodeKind::Dir),
        ("mystery", NodeKind::Symlink),
    ] {
        let n = tree.get(child(&tree, root, name).unwrap()).unwrap();
        assert_eq!(n.kind, kind, "{name}");
    }
    let longjn = child(&tree, root, "longjn").unwrap();
    assert!(tree.get(longjn).unwrap().children.is_empty());
    let cloud = child(&tree, root, "cloud").unwrap();
    assert_eq!(names(&tree, cloud), ["synced.txt"]);

    // The hard link counts once.
    let l1 = tree.get(child(&tree, root, "link1.bin").unwrap()).unwrap();
    let l2 = tree.get(child(&tree, docs, "link2.bin").unwrap()).unwrap();
    assert_eq!(l1.ino, l2.ino);
    assert_eq!(
        u8::from(l1.flags.contains(NodeFlags::HARDLINK_DUP))
            + u8::from(l2.flags.contains(NodeFlags::HARDLINK_DUP)),
        1
    );
    let total = tree.get(root).unwrap().size_allocated;
    assert_eq!(total, 8192 + 12288 + 16 + 40960 + (1 << 30) + 12288 + 4096);

    // The torn record is reported.
    assert_eq!(tree.errors().len(), 1);
    assert!(tree.errors()[0].message.contains("1 MFT records"));
}

#[test]
fn scans_a_subfolder() {
    let img = volume().bytes();
    let root = root_path().join("docs");
    let tree = run_scan(&img, &root, dir_meta(30, 1), ScanOptions::default()).expect("scan");
    assert_eq!(tree.root_path(), root);
    assert_eq!(names(&tree, tree.root()), ["a long name.txt", "link2.bin"]);
    // link2.bin's other name is outside the subtree, so it is counted here.
    assert_eq!(tree.get(tree.root()).unwrap().size_allocated, 12288 + 40960);
}

#[test]
fn honours_skips() {
    let img = volume().bytes();
    let opts = ScanOptions {
        skip: vec![root_path().join("docs")],
        ..ScanOptions::default()
    };
    let tree = run_scan(&img, &root_path(), dir_meta(5, 5), opts).expect("scan");
    let docs = tree
        .get(child(&tree, tree.root(), "docs").unwrap())
        .unwrap();
    assert!(docs.flags.contains(NodeFlags::SKIPPED));
    assert!(docs.children.is_empty());
}

#[test]
fn rejects_a_root_that_does_not_match() {
    let img = volume().bytes();
    // Wrong sequence number: the live folder is not the record we found.
    let err = run_scan(
        &img,
        &root_path().join("docs"),
        dir_meta(30, 2),
        ScanOptions::default(),
    );
    assert!(matches!(err, Err(MftError::RootMismatch)));
    // A file is not a scan root.
    let err = run_scan(&img, &root_path(), dir_meta(31, 3), ScanOptions::default());
    assert!(matches!(err, Err(MftError::RootMismatch)));
}

#[test]
fn follows_mft_extents_listed_in_an_extension_record() {
    // Split the MFT: records 0..32 at LCN 4, the rest at LCN 100. Record 0's
    // own $DATA only maps the first extent; record 15 maps the second.
    let mut img = volume();
    let first = 8u64; // clusters (32 records)
    let total = (img.records.len() * RECORD_SIZE).div_ceil(CLUSTER) as u64;
    let second = total - first;
    let root = fref(5, 5);
    img.records[0] = RecordBuilder::new(1, false)
        .standard(0x06, T)
        .attribute_list(&[0, 15])
        .name(root, 3, "$MFT")
        .data(
            total * CLUSTER as u64,
            (img.records.len() * RECORD_SIZE) as u64,
            &[run(4, first), vec![0]].concat(),
        )
        .build();
    img.records[15] = RecordBuilder::new(1, false)
        .extension_of(0)
        .non_resident(
            0x80,
            "",
            0,
            first,
            0,
            0,
            0,
            &[run(100, second), vec![0]].concat(),
        )
        .build();
    img.extents = vec![(4, first), (100, second)];
    let tree = run_scan(
        &img.bytes(),
        &root_path(),
        dir_meta(5, 5),
        ScanOptions::default(),
    )
    .expect("scan");
    // Records past the first extent (e.g. 33+) were found.
    assert!(child(&tree, tree.root(), "big.iso").is_some());
    assert!(child(&tree, tree.root(), "link1.bin").is_some());
}

#[test]
fn rejects_garbage_volumes() {
    let mut img = volume().bytes();
    img[3] = b'X';
    assert!(matches!(
        run_scan(&img, &root_path(), dir_meta(5, 5), ScanOptions::default()),
        Err(MftError::Format(_))
    ));
    let short = vec![0u8; 100];
    assert!(run_scan(&short, &root_path(), dir_meta(5, 5), ScanOptions::default()).is_err());
}

#[test]
fn cancellation_is_reported() {
    let img = volume().bytes();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let reader: &[u8] = &img;
    let r = scan_volume(
        &reader,
        &root_path(),
        &dir_meta(5, 5),
        &ScanOptions::default(),
        SizeMode::Allocated,
        tx,
        &AtomicBool::new(true),
        Instant::now(),
    );
    assert!(matches!(r, Err(MftError::Cancelled)));
}
