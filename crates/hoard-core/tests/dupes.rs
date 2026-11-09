//! Duplicate finder tests (temp files only).

mod fixtures;

use std::fs;
use std::sync::atomic::AtomicBool;

use fixtures::{find, scan_default, Fixture};
use hoard_core::dupes::{candidates, find_duplicates, DupProgress};

fn write(fx: &Fixture, rel: &str, bytes: &[u8]) {
    let p = fx.path(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

/// 300 KiB of a repeating pattern, with byte `i` changed to `x`.
fn content(x: Option<(usize, u8)>) -> Vec<u8> {
    let mut v: Vec<u8> = (0..300 * 1024).map(|i| (i % 251) as u8).collect();
    if let Some((i, b)) = x {
        v[i] = b;
    }
    v
}

#[test]
fn finds_identical_files_and_rejects_lookalikes() {
    let fx = Fixture::new();
    let same = content(None);
    write(&fx, "a/one.bin", &same);
    write(&fx, "b/two.bin", &same);
    write(&fx, "c/three.bin", &same);
    // Same size, same first and last 64 KiB, different middle: only the full hash catches it.
    write(&fx, "d/middle.bin", &content(Some((150 * 1024, 7))));
    // Different size.
    write(&fx, "e/longer.bin", &[same.as_slice(), b"x"].concat());
    // Small duplicates below the threshold are ignored.
    write(&fx, "small1.txt", b"hello");
    write(&fx, "small2.txt", b"hello");
    // A hard link to one.bin is not a separate copy.
    fs::hard_link(fx.path("a/one.bin"), fx.path("a/link.bin")).unwrap();

    let tree = scan_default(fx.root());
    let mut stages = Vec::new();
    let result = find_duplicates(
        candidates(&tree, 1024),
        &AtomicBool::new(false),
        &mut |p: DupProgress| stages.push(p.stage),
    );

    assert!(!result.cancelled);
    assert_eq!(result.groups.len(), 1, "{:?}", result.groups);
    let g = &result.groups[0];
    let mut names: Vec<String> = g
        .nodes
        .iter()
        .map(|&n| tree.get(n).unwrap().display_name())
        .collect();
    names.sort();
    // one.bin or its link (whichever the scan counted first) plus the other two.
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names.contains(&"two.bin".to_string()) && names.contains(&"three.bin".to_string()));
    assert_eq!(g.size, same.len() as u64);
    assert_eq!(g.wasted(), 2 * same.len() as u64);
    assert_eq!(g.hash.len(), 64);
    assert!(stages.contains(&1) && stages.contains(&2));
}

#[test]
fn small_files_are_compared_completely() {
    // Under 128 KiB the partial hash reads the whole file.
    let fx = Fixture::new();
    write(&fx, "x.bin", &[1u8; 5000]);
    write(&fx, "y.bin", &[1u8; 5000]);
    let mut z = vec![1u8; 5000];
    z[2500] = 2;
    write(&fx, "z.bin", &z);
    let tree = scan_default(fx.root());
    let result = find_duplicates(candidates(&tree, 1), &AtomicBool::new(false), &mut |_| {});
    assert_eq!(result.groups.len(), 1);
    let ids = &result.groups[0].nodes;
    assert!(
        ids.contains(&find(&tree, "x.bin").unwrap())
            && ids.contains(&find(&tree, "y.bin").unwrap())
    );
}

#[test]
fn removed_files_are_not_candidates_and_vanished_files_are_skipped() {
    let fx = Fixture::new();
    let same = content(None);
    write(&fx, "a.bin", &same);
    write(&fx, "b.bin", &same);
    write(&fx, "c.bin", &same);
    let mut tree = scan_default(fx.root());
    tree.remove_subtree(find(&tree, "a.bin").unwrap());
    let cands = candidates(&tree, 1);
    assert_eq!(cands.len(), 2);
    // Deleted from disk after the scan: unreadable, not fatal.
    fs::remove_file(fx.path("c.bin")).unwrap();
    let result = find_duplicates(cands, &AtomicBool::new(false), &mut |_| {});
    assert!(result.groups.is_empty());
    assert_eq!(result.unreadable, 1);
}

#[test]
fn cancel_stops_the_job() {
    let fx = Fixture::new();
    let same = content(None);
    for i in 0..10 {
        write(&fx, &format!("f{i}.bin"), &same);
    }
    let tree = scan_default(fx.root());
    let result = find_duplicates(candidates(&tree, 1), &AtomicBool::new(true), &mut |_| {});
    assert!(result.cancelled);
    assert!(result.groups.is_empty());
}
