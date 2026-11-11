//! Scanner integration tests (M0 required cases).

mod fixtures;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use fixtures::{find, scan_default, scan_with, Fixture};
use hoard_core::{scan, NodeFlags, NodeKind, ScanErrorKind, ScanEvent, ScanOptions, SizeMode};

#[test]
fn totals_match_expected_sizes() {
    let fx = Fixture::new();
    let sizes = [
        ("a.bin", 10_000usize),
        ("d1/b.bin", 123_457),
        ("d1/d2/c.bin", 1_048_576),
        ("d1/d2/empty", 0),
    ];
    for (rel, size) in sizes {
        fx.file(rel, size);
    }
    fx.dir("d3/empty_dir");

    let tree = scan_default(fx.root());
    assert!(tree.is_complete());
    assert!(tree.errors().is_empty(), "{:?}", tree.errors());

    let expected: u64 = sizes.iter().map(|(_, s)| *s as u64).sum();
    assert_eq!(tree.total(SizeMode::Apparent), expected);
    let root = tree.get(tree.root()).unwrap();
    assert_eq!(root.file_count, 4);
    assert_eq!(tree.dir_count(), 5); // root, d1, d2, d3, empty_dir

    for (rel, size) in sizes {
        let node = tree.get(find(&tree, rel).unwrap()).unwrap();
        assert_eq!(node.kind, NodeKind::File);
        assert_eq!(node.size_apparent, size as u64, "{rel}");
        // Allocated is cluster rounded: at least the apparent size, at most
        // one large cluster more, in 512-byte units.
        assert!(node.size_allocated >= node.size_apparent, "{rel}: {node:?}");
        assert!(
            node.size_allocated - node.size_apparent < 64 * 1024,
            "{rel}: {node:?}"
        );
        assert_eq!(node.size_allocated % 512, 0, "{rel}");
    }
    assert_eq!(
        tree.total(SizeMode::Allocated),
        independent_allocated_total(fx.root())
    );
}

/// Allocated total computed without the scanner.
#[cfg(unix)]
fn independent_allocated_total(root: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::symlink_metadata(root).unwrap();
    let mut total = md.blocks() * 512;
    if md.is_dir() {
        for e in std::fs::read_dir(root).unwrap() {
            total += independent_allocated_total(&e.unwrap().path());
        }
    }
    total
}

/// On Windows, allocated sizes come from the same API the scanner uses, so
/// check aggregation consistency instead: the root equals the sum of its leaves.
#[cfg(windows)]
fn independent_allocated_total(root: &std::path::Path) -> u64 {
    let tree = scan_default(root);
    (0..tree.len() as u32)
        .filter_map(|i| tree.get(hoard_core::NodeId(i)))
        .map(|n| {
            if n.is_dir() {
                own_dir_allocated(&tree, n)
            } else {
                n.size_allocated
            }
        })
        .sum()
}

#[cfg(windows)]
fn own_dir_allocated(tree: &hoard_core::Tree, n: &hoard_core::Node) -> u64 {
    let children: u64 = n
        .children
        .iter()
        .map(|c| tree.get(*c).unwrap().size_allocated)
        .sum();
    n.size_allocated - children
}

#[test]
fn hardlink_counted_once() {
    let fx = Fixture::new();
    fx.file("orig/data.bin", 200_000);
    fx.hardlink("orig/data.bin", "other/link.bin");
    fx.file("other/plain.bin", 1_000);

    let tree = scan_default(fx.root());
    assert_eq!(tree.total(SizeMode::Apparent), 201_000);

    let a = tree.get(find(&tree, "orig/data.bin").unwrap()).unwrap();
    let b = tree.get(find(&tree, "other/link.bin").unwrap()).unwrap();
    let dups = [a, b]
        .iter()
        .filter(|n| n.flags.contains(NodeFlags::HARDLINK_DUP))
        .count();
    assert_eq!(dups, 1, "exactly one link is the duplicate");
    assert_eq!(a.ino, b.ino);
    assert_eq!(tree.get(tree.root()).unwrap().file_count, 3);
}

#[test]
fn symlink_to_outside_root_not_followed() {
    let outside = Fixture::new();
    outside.file("huge.bin", 2 * 1024 * 1024);
    let fx = Fixture::new();
    fx.file("small.bin", 1_000);
    let kind = fx.dir_link(outside.root(), "link_out");

    let tree = scan_default(fx.root());
    assert!(tree.errors().is_empty(), "{:?}", tree.errors());
    assert_eq!(
        tree.total(SizeMode::Apparent),
        1_000,
        "link target counted ({kind:?})"
    );
    let link = tree.get(find(&tree, "link_out").unwrap()).unwrap();
    assert_eq!(link.kind, NodeKind::Symlink, "{kind:?}");
    assert!(link.children.is_empty());
    assert_eq!(tree.get(tree.root()).unwrap().file_count, 2);
}

#[test]
fn file_symlink_not_followed() {
    let outside = Fixture::new();
    let target = outside.file("target.bin", 500_000);
    let fx = Fixture::new();
    if !fx.file_symlink(&target, "flink") {
        eprintln!("skipped: file symlinks need Developer Mode or admin on Windows");
        return;
    }
    let tree = scan_default(fx.root());
    let link = tree.get(find(&tree, "flink").unwrap()).unwrap();
    assert_eq!(link.kind, NodeKind::Symlink);
    assert!(
        link.size_apparent < 1_000,
        "link size is the link, not the target"
    );
}

#[test]
fn symlink_loop_does_not_hang() {
    let fx = Fixture::new();
    fx.file("a/b/file.bin", 4_096);
    // Link back up to the root and to its own parent.
    fx.dir_link(fx.root(), "a/b/to_root");
    fx.dir_link(&fx.path("a"), "a/b/to_parent");
    #[cfg(unix)]
    {
        // Two links pointing at each other.
        fx.file_symlink(&fx.path("y"), "x");
        fx.file_symlink(&fx.path("x"), "y");
    }

    let root = fx.root().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let tree = scan_default(&root);
        let _ = tx.send((tree.total(SizeMode::Apparent), tree.is_complete()));
    });
    let (apparent, complete) = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("scan hung on a link loop");
    assert!(complete);
    assert!(apparent < 10_000, "followed a loop: {apparent}");
}

#[test]
fn permission_denied_dir_recorded_not_fatal() {
    let fx = Fixture::new();
    fx.file("readable/ok.bin", 5_000);
    fx.file("locked/secret.bin", 50_000);
    let Some(_guard) = fx.unreadable_dir("locked") else {
        eprintln!("skipped: cannot create an unreadable directory as this user");
        return;
    };

    let tree = scan_default(fx.root());
    assert!(tree.is_complete());
    let locked_path = fx.path("locked");
    let err = tree
        .errors()
        .iter()
        .find(|e| e.path == locked_path)
        .unwrap_or_else(|| panic!("no error recorded for locked dir: {:?}", tree.errors()));
    assert_eq!(err.kind, ScanErrorKind::ReadDir);
    let locked = tree.get(find(&tree, "locked").unwrap()).unwrap();
    assert!(locked.flags.contains(NodeFlags::ERROR));
    assert!(locked.children.is_empty());
    // The readable part is still fully counted.
    assert_eq!(
        tree.get(find(&tree, "readable").unwrap())
            .unwrap()
            .size_apparent,
        5_000
    );
}

#[test]
fn root_symlink_is_not_followed() {
    let outside = Fixture::new();
    outside.file("big.bin", 1_000_000);
    let fx = Fixture::new();
    fx.dir_link(outside.root(), "root_link");

    let tree = scan_default(&fx.path("root_link"));
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.get(tree.root()).unwrap().kind, NodeKind::Symlink);
    assert_eq!(tree.total(SizeMode::Apparent), 0);
}

#[test]
fn missing_root_is_an_error() {
    let fx = Fixture::new();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let res = scan(
        &fx.path("nope"),
        ScanOptions::default(),
        tx,
        Arc::new(AtomicBool::new(false)),
    );
    assert!(res.is_err());
}

#[test]
fn single_file_root() {
    let fx = Fixture::new();
    let f = fx.file("one.bin", 777);
    let tree = scan_default(&f);
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.total(SizeMode::Apparent), 777);
}

#[test]
fn cancelled_scan_returns_partial_tree() {
    let fx = Fixture::new();
    for i in 0..50 {
        fx.file(&format!("d{i}/f.bin"), 100);
    }
    let (tx, _rx) = crossbeam_channel::unbounded();
    let cancel = Arc::new(AtomicBool::new(true));
    let tree = scan(fx.root(), ScanOptions::default(), tx, cancel).unwrap();
    assert!(!tree.is_complete());
    assert!(tree.get(tree.root()).unwrap().file_count < 50);
}

#[test]
fn skip_option_keeps_node_but_does_not_descend() {
    let fx = Fixture::new();
    fx.file("keep/a.bin", 1_000);
    fx.file("skip_me/b.bin", 9_000);
    let opts = ScanOptions {
        skip: vec![fx.path("skip_me")],
        ..ScanOptions::default()
    };
    let (tree, _) = scan_with(fx.root(), opts);
    assert_eq!(tree.total(SizeMode::Apparent), 1_000);
    let skipped = tree.get(find(&tree, "skip_me").unwrap()).unwrap();
    assert!(skipped.flags.contains(NodeFlags::SKIPPED));
    assert!(skipped.children.is_empty());
}

#[test]
fn paths_round_trip() {
    let fx = Fixture::new();
    let f = fx.file("x/y/z/deep.txt", 10);
    let tree = scan_default(fx.root());
    let id = find(&tree, "x/y/z/deep.txt").unwrap();
    assert_eq!(tree.path(id).unwrap(), f);
    assert_eq!(tree.path(tree.root()).unwrap(), fx.root());
}

#[test]
fn final_progress_and_batch_events_are_sent() {
    let fx = Fixture::new();
    fx.file("a/1.bin", 1_000);
    fx.file("a/2.bin", 2_000);
    fx.file("b.bin", 4_000);
    let opts = ScanOptions {
        apparent: true,
        ..ScanOptions::default()
    };
    let (tree, events) = scan_with(fx.root(), opts);
    assert_eq!(tree.size_mode(), SizeMode::Apparent);

    let last_progress = events.iter().rev().find_map(|e| match e {
        ScanEvent::Progress { files, bytes, .. } => Some((*files, *bytes)),
        _ => None,
    });
    assert_eq!(last_progress, Some((3, 7_000)));

    let ScanEvent::Batch { top_level } = events.last().unwrap() else {
        panic!("last event should be a batch");
    };
    let names: Vec<(&str, u64)> = top_level
        .iter()
        .map(|t| (t.name.as_str(), t.size_apparent))
        .collect();
    assert_eq!(names, vec![("b.bin", 4_000), ("a", 3_000)]);
}

#[cfg(windows)]
#[test]
fn long_paths_are_scanned() {
    let fx = Fixture::new();
    let segment = "d".repeat(60);
    let rel: String = (0..6)
        .map(|_| segment.as_str())
        .collect::<Vec<_>>()
        .join("/")
        + "/f.bin";
    let full = fx.path(&rel);
    assert!(full.as_os_str().len() > 300);
    fx.file(&rel, 12_345);
    let tree = scan_default(fx.root());
    assert!(tree.errors().is_empty(), "{:?}", tree.errors());
    assert_eq!(tree.total(SizeMode::Apparent), 12_345);
}

/// The per-directory fast path must describe every kind of entry exactly as
/// the plain per-path `stat` does: same kind, sizes, times, identity and link
/// count, and links described, never followed.
#[test]
fn dir_stat_matches_stat() {
    use hoard_core::platform::{self, DirStat};

    let fx = Fixture::new();
    fx.file("d/small.bin", 1);
    fx.file("d/big.bin", 300_000);
    fx.file("d/empty", 0);
    fx.dir("d/sub/inner");
    fx.file("d/sub/inner/x.bin", 5_000);
    fx.file("d/linked.bin", 70_000);
    fx.hardlink("d/linked.bin", "d/linked2.bin");
    fx.file("outside/payload.bin", 2_000_000);
    fx.dir_link(&fx.path("outside"), "d/dirlink");
    fx.file_symlink(&fx.path("outside/payload.bin"), "d/filelink");
    fx.file("d/unicode \u{e9}\u{4e2d}.txt", 10);
    #[cfg(windows)]
    {
        // Always cover a junction too (dir_link prefers a real symlink).
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(fx.path("d/junction"))
            .arg(fx.path("outside"))
            .stdout(std::process::Stdio::null())
            .status()
            .expect("run mklink");
        assert!(status.success());
        // A compressed file takes the fallback for its allocated size.
        fx.file("d/compressed.txt", 200_000);
        let _ = std::process::Command::new("compact")
            .args(["/c", "/q"])
            .arg(fx.path("d/compressed.txt"))
            .stdout(std::process::Stdio::null())
            .status();
    }

    let dir = fx.path("d");
    let ds = DirStat::open(&dir);
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("list") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let fast = ds.stat(&entry.file_name(), &path).expect("fast stat");
        let slow = platform::stat(&path).expect("stat");
        assert_eq!(fast, slow, "{}", path.display());
        checked += 1;
    }
    assert!(checked >= 9, "only {checked} entries checked");
    // The comparison above must have exercised the fast path, not only its
    // fallback (NTFS temp dirs support it on every supported Windows).
    #[cfg(windows)]
    assert!(ds.is_fast());

    let linked = ds
        .stat("linked.bin".as_ref(), &fx.path("d/linked.bin"))
        .expect("stat");
    assert_eq!(linked.nlink, 2);
    let link = ds
        .stat("dirlink".as_ref(), &fx.path("d/dirlink"))
        .expect("stat");
    assert_eq!(link.kind, NodeKind::Symlink);
    #[cfg(windows)]
    {
        let j = ds
            .stat("junction".as_ref(), &fx.path("d/junction"))
            .expect("stat");
        assert_eq!(j.kind, NodeKind::Symlink);
    }
}

/// A directory that cannot be opened still stats its entries (by full path),
/// and missing entries report errors rather than inventing data.
#[test]
fn dir_stat_falls_back() {
    use hoard_core::platform::{self, DirStat};

    let fx = Fixture::new();
    fx.file("a.bin", 4_000);
    let gone = DirStat::open(&fx.path("no-such-dir"));
    assert!(!gone.is_fast());
    let meta = gone
        .stat("a.bin".as_ref(), &fx.path("a.bin"))
        .expect("fallback");
    assert_eq!(meta, platform::stat(&fx.path("a.bin")).expect("stat"));

    let ds = DirStat::open(fx.root());
    assert!(ds.stat("missing".as_ref(), &fx.path("missing")).is_err());
}

/// A large directory has a non-resident index with its own allocation. The
/// scan must count it exactly as a per-entry `stat` does, also on Windows
/// where the fast path takes it from the listing of the directory itself.
#[test]
fn scan_totals_match_per_entry_stat() {
    use hoard_core::platform::{self, DirStat};

    let fx = Fixture::new();
    for i in 0..1500 {
        fx.file(
            &format!("big/a-rather-long-file-name-number-{i:05}.txt"),
            10,
        );
    }
    fx.file("big/nested/x.bin", 50_000);
    fx.file("small/y.bin", 7_000);

    let big = platform::stat(&fx.path("big")).expect("stat");
    if cfg!(windows) {
        assert!(big.size_allocated > 0, "index should be non-resident");
    }
    assert_eq!(
        DirStat::open(&fx.path("big"))
            .own_allocated()
            .unwrap_or(big.size_allocated),
        big.size_allocated
    );

    let mut expected = 0u64;
    let mut stack = vec![fx.root().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("list") {
            let p = e.expect("entry").path();
            let m = platform::stat(&p).expect("stat");
            expected += m.size_allocated;
            if m.kind == NodeKind::Dir {
                stack.push(p);
            }
        }
    }
    expected += platform::stat(fx.root()).expect("stat").size_allocated;

    let tree = scan_default(fx.root());
    assert_eq!(
        tree.get(tree.root()).expect("root").size_allocated,
        expected
    );
    let big_id = find(&tree, "big").expect("big");
    assert!(tree.get(big_id).expect("big").size_allocated >= big.size_allocated + 1500 * 10);
}
