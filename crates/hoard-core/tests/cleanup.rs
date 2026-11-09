//! Cleanup safety tests (M2). Every test works inside temp dirs only.
//!
//! Required by CLAUDE.md before M2 is done:
//! - symlink to outside root deletes only the link
//! - protected path rejected
//! - item swapped after scan (different inode) rejected
//! - ancestor of scan root rejected
//! - partial failure leaves the rest of the tray processed and reported

mod fixtures;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, RwLock};

use fixtures::{find, scan_default, Fixture};
use hoard_core::cleanup::{
    protected, requires_typed_confirmation, run_cleanup, CleanupContext, CleanupError, CleanupMode,
    CleanupReport, ItemOutcome, Remover, SkipReason, Tray,
};
use hoard_core::{NodeFlags, NodeId, NodeKind, SizeMode, Tree};

/// A stand-in for the Recycle Bin: moves items into a separate temp dir.
struct FakeTrash {
    bin: tempfile::TempDir,
    calls: Mutex<Vec<PathBuf>>,
    /// If true, pretend to succeed without moving anything.
    lie: bool,
}

impl FakeTrash {
    fn new() -> Self {
        FakeTrash {
            bin: tempfile::tempdir().unwrap(),
            calls: Mutex::new(Vec::new()),
            lie: false,
        }
    }
}

impl Remover for FakeTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(path.to_path_buf());
        if self.lie {
            return Ok(());
        }
        let dest = self.bin.path().join(format!(
            "{}-{}",
            calls.len(),
            path.file_name().unwrap().to_string_lossy()
        ));
        fs::rename(path, dest).map_err(|e| e.to_string())
    }
}

struct Setup {
    fx: Fixture,
    tree: RwLock<Tree>,
    ctx: CleanupContext,
    _logs: tempfile::TempDir,
    log_path: PathBuf,
}

/// Scans `fx.root()` and builds a context whose action log lives in its own temp dir.
fn setup(fx: Fixture) -> Setup {
    let tree = scan_default(fx.root());
    let logs = tempfile::tempdir().unwrap();
    let log_path = logs.path().join("actions.jsonl");
    let ctx = CleanupContext::new(tree.root_path().to_path_buf(), log_path.clone());
    Setup {
        fx,
        tree: RwLock::new(tree),
        ctx,
        _logs: logs,
        log_path,
    }
}

impl Setup {
    fn id(&self, rel: &str) -> NodeId {
        find(&self.tree.read().unwrap(), rel).unwrap_or_else(|| panic!("{rel} not in tree"))
    }

    fn run(&self, ids: &[NodeId], mode: CleanupMode) -> CleanupReport {
        self.run_with(ids, mode, &FakeTrash::new(), None)
            .expect("cleanup run")
    }

    fn run_with(
        &self,
        ids: &[NodeId],
        mode: CleanupMode,
        remover: &dyn Remover,
        confirmation: Option<&str>,
    ) -> Result<CleanupReport, CleanupError> {
        run_cleanup(
            &self.tree,
            ids,
            mode,
            confirmation,
            &self.ctx,
            remover,
            "test-run",
            &AtomicBool::new(false),
            &mut |_| {},
        )
    }

    fn log_lines(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(&self.log_path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("log line is JSON"))
            .collect()
    }
}

fn outcome(report: &CleanupReport, id: NodeId) -> &ItemOutcome {
    &report
        .results
        .iter()
        .find(|r| r.id == id)
        .expect("result for item")
        .outcome
}

// ---------------------------------------------------------------- required

#[test]
fn symlink_to_outside_root_deletes_only_the_link() {
    let outside = Fixture::new();
    let precious = outside.file("precious/data.bin", 50_000);
    let fx = Fixture::new();
    fx.file("keep.bin", 10);
    let kind = fx.dir_link(&outside.path("precious"), "link_out");
    let s = setup(fx);
    let link = s.id("link_out");
    assert_eq!(
        s.tree.read().unwrap().get(link).unwrap().kind,
        NodeKind::Symlink,
        "{kind:?}"
    );

    let report = s.run(&[link], CleanupMode::Permanent);
    assert!(
        matches!(outcome(&report, link), ItemOutcome::Removed),
        "{report:?}"
    );
    assert!(
        fs::symlink_metadata(s.fx.path("link_out")).is_err(),
        "link should be gone"
    );
    assert!(precious.exists(), "link target must survive");
    assert_eq!(fs::read(&precious).unwrap().len(), 50_000);
}

#[test]
fn symlink_inside_a_deleted_folder_is_unlinked_not_followed() {
    let outside = Fixture::new();
    let precious = outside.file("precious/data.bin", 20_000);
    let fx = Fixture::new();
    fx.file("victim/a.bin", 1_000);
    fx.dir_link(&outside.path("precious"), "victim/link_out");
    let s = setup(fx);
    let victim = s.id("victim");

    let report = s.run(&[victim], CleanupMode::Permanent);
    assert!(
        matches!(outcome(&report, victim), ItemOutcome::Removed),
        "{report:?}"
    );
    assert!(!s.fx.path("victim").exists());
    assert!(
        precious.exists(),
        "target behind a nested link must survive"
    );
}

#[test]
fn protected_paths_are_rejected() {
    let fx = Fixture::new();
    fx.file("fakehome/Documents/cv.pdf", 100);
    fx.file("fakehome/notes.txt", 100);
    let mut s = setup(fx);
    s.ctx = s
        .ctx
        .clone()
        .with_home(s.fx.path("fakehome"))
        .with_user_dirs(vec![s.fx.path("fakehome/Documents")]);
    let home = s.id("fakehome");
    let docs = s.id("fakehome/Documents");
    let cv = s.id("fakehome/Documents/cv.pdf");

    let report = s.run(&[home, docs, cv], CleanupMode::Permanent);
    assert!(
        matches!(
            outcome(&report, home),
            ItemOutcome::Skipped(SkipReason::Protected { .. })
        ),
        "{report:?}"
    );
    assert!(
        matches!(
            outcome(&report, docs),
            ItemOutcome::Skipped(SkipReason::Protected { .. })
        ),
        "{report:?}"
    );
    // Contents of a top-level user folder are fair game; the folder itself is not.
    assert!(
        matches!(outcome(&report, cv), ItemOutcome::Removed),
        "{report:?}"
    );
    assert!(s.fx.path("fakehome/notes.txt").exists());
    assert!(s.fx.path("fakehome/Documents").exists());
}

#[test]
fn item_swapped_after_scan_is_rejected() {
    let fx = Fixture::new();
    fx.file("swap.bin", 4_000);
    let s = setup(fx);
    let id = s.id("swap.bin");

    // Replace the file with a different one at the same path.
    let path = s.fx.path("swap.bin");
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"new and important").unwrap();

    let report = s.run(&[id], CleanupMode::Permanent);
    assert!(
        matches!(
            outcome(&report, id),
            ItemOutcome::Skipped(SkipReason::Changed { .. })
        ),
        "{report:?}"
    );
    assert_eq!(fs::read(&path).unwrap(), b"new and important");
}

#[test]
fn ancestor_of_scan_root_is_rejected() {
    let fx = Fixture::new();
    fx.file("project/sub/file.bin", 10);
    // Scan a subfolder, so the fixture root is an ancestor of the scan root.
    let tree = scan_default(&fx.path("project/sub"));
    let ctx = CleanupContext::new(tree.root_path().to_path_buf(), fx.path("log.jsonl"));
    assert!(protected::check(&fx.path("project"), &ctx).is_some());
    assert!(protected::check(fx.root(), &ctx).is_some());
    // The scan root itself is rejected too (decision: never delete the scanned folder).
    let tree = RwLock::new(tree);
    let root = tree.read().unwrap().root();
    let report = run_cleanup(
        &tree,
        &[root],
        CleanupMode::Permanent,
        None,
        &ctx,
        &FakeTrash::new(),
        "r",
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert!(
        matches!(
            outcome(&report, root),
            ItemOutcome::Skipped(SkipReason::ScanRoot)
        ),
        "{report:?}"
    );
    assert!(fx.path("project/sub/file.bin").exists());
}

#[test]
fn partial_failure_leaves_the_rest_processed_and_reported() {
    let fx = Fixture::new();
    fx.file("a.bin", 1_000);
    fx.file("b.bin", 2_000);
    fx.file("c.bin", 3_000);
    let s = setup(fx);
    let (a, b, c) = (s.id("a.bin"), s.id("b.bin"), s.id("c.bin"));
    // b changes underneath us.
    fs::remove_file(s.fx.path("b.bin")).unwrap();
    fs::write(s.fx.path("b.bin"), b"replacement").unwrap();

    let report = s.run(&[a, b, c], CleanupMode::Permanent);
    assert!(matches!(outcome(&report, a), ItemOutcome::Removed));
    assert!(matches!(outcome(&report, b), ItemOutcome::Skipped(_)));
    assert!(matches!(outcome(&report, c), ItemOutcome::Removed));
    assert!(!s.fx.path("a.bin").exists());
    assert!(s.fx.path("b.bin").exists());
    assert!(!s.fx.path("c.bin").exists());

    let lines = s.log_lines();
    assert_eq!(lines.len(), 3, "one log line per item: {lines:?}");
    let results: Vec<&str> = lines
        .iter()
        .map(|l| l["result"].as_str().unwrap())
        .collect();
    assert_eq!(results, vec!["removed", "skipped", "removed"]);
    assert!(lines
        .iter()
        .all(|l| l["run_id"] == "test-run" && l["mode"] == "permanent"));
    assert!(lines[1]["reason"].as_str().is_some_and(|r| !r.is_empty()));
}

// ---------------------------------------------------------------- tree + modes

#[test]
fn tree_is_updated_in_place() {
    let fx = Fixture::new();
    fx.file("big/x.bin", 100_000);
    fx.file("keep.bin", 5_000);
    let s = setup(fx);
    let big = s.id("big");
    let before = s.tree.read().unwrap().total(SizeMode::Apparent);

    let report = s.run(&[big], CleanupMode::Permanent);
    assert!(matches!(outcome(&report, big), ItemOutcome::Removed));
    assert_eq!(report.estimated_freed_apparent, 100_000);

    let tree = s.tree.read().unwrap();
    assert_eq!(tree.total(SizeMode::Apparent), before - 100_000);
    assert!(tree.get(big).unwrap().flags.contains(NodeFlags::REMOVED));
    assert!(find(&tree, "big").is_none());
    assert_eq!(tree.get(tree.root()).unwrap().file_count, 1);
}

#[test]
fn trash_mode_uses_the_remover_and_verifies_the_item_is_gone() {
    let fx = Fixture::new();
    fx.file("old.iso", 30_000);
    let s = setup(fx);
    let id = s.id("old.iso");
    let bin = FakeTrash::new();
    let report = s.run_with(&[id], CleanupMode::Trash, &bin, None).unwrap();
    assert!(
        matches!(outcome(&report, id), ItemOutcome::Removed),
        "{report:?}"
    );
    assert_eq!(bin.calls.lock().unwrap().len(), 1);
    assert!(!s.fx.path("old.iso").exists());
    assert_eq!(s.log_lines()[0]["mode"], "trash");
}

#[test]
fn trash_that_reports_success_but_leaves_the_item_is_a_failure() {
    let fx = Fixture::new();
    fx.file("stubborn.bin", 1_000);
    let s = setup(fx);
    let id = s.id("stubborn.bin");
    let bin = FakeTrash {
        lie: true,
        ..FakeTrash::new()
    };
    let report = s.run_with(&[id], CleanupMode::Trash, &bin, None).unwrap();
    assert!(
        matches!(outcome(&report, id), ItemOutcome::Failed(_)),
        "{report:?}"
    );
    // Never fall back to a weaker method.
    assert!(s.fx.path("stubborn.bin").exists());
    assert!(!s
        .tree
        .read()
        .unwrap()
        .get(id)
        .unwrap()
        .flags
        .contains(NodeFlags::REMOVED));
}

#[test]
fn deleted_after_scan_is_skipped_not_failed() {
    let fx = Fixture::new();
    fx.file("gone.bin", 10);
    let s = setup(fx);
    let id = s.id("gone.bin");
    fs::remove_file(s.fx.path("gone.bin")).unwrap();
    let report = s.run(&[id], CleanupMode::Permanent);
    assert!(
        matches!(
            outcome(&report, id),
            ItemOutcome::Skipped(SkipReason::Changed { .. })
        ),
        "{report:?}"
    );
}

#[test]
fn folders_with_unknown_contents_are_rejected() {
    let fx = Fixture::new();
    fx.file("locked/secret.bin", 100);
    fx.file("skipme/x.bin", 100);
    let guard = fx.unreadable_dir("locked");
    let opts = hoard_core::ScanOptions {
        skip: vec![fx.path("skipme")],
        ..Default::default()
    };
    let tree = fixtures::scan_with(fx.root(), opts).0;
    drop(guard);
    let logs = tempfile::tempdir().unwrap();
    let ctx = CleanupContext::new(tree.root_path().to_path_buf(), logs.path().join("l.jsonl"));
    let skip = find(&tree, "skipme").unwrap();
    let locked = find(&tree, "locked").unwrap();
    let had_guard = tree.get(locked).unwrap().flags.contains(NodeFlags::ERROR);
    let tree = RwLock::new(tree);
    let report = run_cleanup(
        &tree,
        &[skip, locked],
        CleanupMode::Permanent,
        None,
        &ctx,
        &FakeTrash::new(),
        "r",
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert!(
        matches!(
            outcome(&report, skip),
            ItemOutcome::Skipped(SkipReason::ContentsUnknown)
        ),
        "{report:?}"
    );
    assert!(fx.path("skipme/x.bin").exists());
    if had_guard {
        assert!(
            matches!(
                outcome(&report, locked),
                ItemOutcome::Skipped(SkipReason::ContentsUnknown)
            ),
            "{report:?}"
        );
        assert!(fx.path("locked/secret.bin").exists());
    }
}

#[test]
fn items_covered_by_another_item_are_skipped() {
    let fx = Fixture::new();
    fx.file("dir/inner.bin", 100);
    let s = setup(fx);
    let dir = s.id("dir");
    let inner = s.id("dir/inner.bin");
    let report = s.run(&[inner, dir], CleanupMode::Permanent);
    assert!(
        matches!(outcome(&report, dir), ItemOutcome::Removed),
        "{report:?}"
    );
    assert!(
        matches!(
            outcome(&report, inner),
            ItemOutcome::Skipped(SkipReason::CoveredByOther)
        ),
        "{report:?}"
    );
}

#[test]
fn cancel_skips_remaining_items() {
    let fx = Fixture::new();
    fx.file("a.bin", 10);
    fx.file("b.bin", 10);
    let s = setup(fx);
    let (a, b) = (s.id("a.bin"), s.id("b.bin"));
    let cancel = AtomicBool::new(true);
    let report = run_cleanup(
        &s.tree,
        &[a, b],
        CleanupMode::Permanent,
        None,
        &s.ctx,
        &FakeTrash::new(),
        "r",
        &cancel,
        &mut |_| {},
    )
    .unwrap();
    assert!(report.cancelled);
    assert!(report
        .results
        .iter()
        .all(|r| matches!(r.outcome, ItemOutcome::Skipped(SkipReason::Cancelled))));
    assert!(s.fx.path("a.bin").exists() && s.fx.path("b.bin").exists());
}

// ---------------------------------------------------------------- log + confirmation

#[test]
fn unwritable_log_aborts_before_anything_is_deleted() {
    let fx = Fixture::new();
    fx.file("a.bin", 10);
    let mut s = setup(fx);
    // A directory where the log file should be: opening it for append fails.
    let blocker = s.fx.path("log_is_a_dir");
    fs::create_dir_all(&blocker).unwrap();
    s.ctx = CleanupContext::new(s.ctx.scan_root.clone(), blocker);
    let id = s.id("a.bin");
    let err = s
        .run_with(&[id], CleanupMode::Permanent, &FakeTrash::new(), None)
        .unwrap_err();
    assert!(matches!(err, CleanupError::Log(_)), "{err:?}");
    assert!(s.fx.path("a.bin").exists());
}

#[test]
fn typed_confirmation_thresholds() {
    const GIB: u64 = 1024 * 1024 * 1024;
    assert_eq!(
        requires_typed_confirmation(CleanupMode::Trash, 500 * GIB, 50_000),
        None
    );
    assert_eq!(
        requires_typed_confirmation(CleanupMode::Permanent, GIB, 10),
        None
    );
    assert_eq!(
        requires_typed_confirmation(CleanupMode::Permanent, 10 * GIB, 1000),
        None
    );
    assert_eq!(
        requires_typed_confirmation(CleanupMode::Permanent, 10 * GIB + 1, 1),
        Some("10.0 GB".into())
    );
    assert_eq!(
        requires_typed_confirmation(CleanupMode::Permanent, 2048, 1001),
        Some("2.0 KB".into())
    );
}

#[test]
fn permanent_over_threshold_requires_the_typed_size() {
    let fx = Fixture::new();
    for i in 0..1001 {
        fx.file(&format!("many/f{i}.bin"), 0);
    }
    let s = setup(fx);
    let many = s.id("many");
    let err = s
        .run_with(&[many], CleanupMode::Permanent, &FakeTrash::new(), None)
        .unwrap_err();
    let CleanupError::ConfirmationRequired { expected } = err else {
        panic!("expected confirmation error, got {err:?}");
    };
    assert!(s.fx.path("many").exists());
    let wrong = s.run_with(
        &[many],
        CleanupMode::Permanent,
        &FakeTrash::new(),
        Some("1 KB"),
    );
    assert!(matches!(
        wrong,
        Err(CleanupError::ConfirmationRequired { .. })
    ));
    let report = s
        .run_with(
            &[many],
            CleanupMode::Permanent,
            &FakeTrash::new(),
            Some(&expected),
        )
        .unwrap();
    assert!(
        matches!(outcome(&report, many), ItemOutcome::Removed),
        "{report:?}"
    );
    assert!(!s.fx.path("many").exists());
}

// ---------------------------------------------------------------- tray

#[test]
fn tray_dedupes_nested_items_and_rejects_unsafe_ones() {
    let fx = Fixture::new();
    fx.file("dir/a.bin", 100);
    fx.file("dir/b.bin", 200);
    fx.file("other.bin", 300);
    let s = setup(fx);
    let tree = s.tree.read().unwrap();
    let (dir, a, b, other) = (
        s.id("dir"),
        s.id("dir/a.bin"),
        s.id("dir/b.bin"),
        s.id("other.bin"),
    );
    let mut tray = Tray::new(1);
    let v0 = tray.version();

    let out = tray.add(&tree, &[a, b], &s.ctx);
    assert_eq!(out.added, vec![a, b]);
    assert!(tray.version() > v0);

    // Adding the parent replaces its children.
    let out = tray.add(&tree, &[dir], &s.ctx);
    assert_eq!(out.added, vec![dir]);
    assert_eq!(out.replaced, vec![a, b]);
    assert_eq!(tray.items(), &[dir]);

    // A child of something already in the tray is covered.
    let out = tray.add(&tree, &[a, dir], &s.ctx);
    assert!(out.added.is_empty());
    assert_eq!(out.covered, vec![a, dir]);

    // The scan root is rejected.
    let out = tray.add(&tree, &[tree.root()], &s.ctx);
    assert!(matches!(out.rejected[0].1, SkipReason::ScanRoot));

    tray.add(&tree, &[other], &s.ctx);
    assert_eq!(tray.total(&tree, SizeMode::Apparent), 600);
    tray.remove(&[dir]);
    assert_eq!(tray.items(), &[other]);
    let v = tray.version();
    tray.clear();
    assert!(tray.items().is_empty() && tray.version() > v);
    assert_eq!(tray.generation(), 1);
}

// ---------------------------------------------------------------- real trash (manual)

/// Moves a temp file into the real Recycle Bin / Trash. Run manually:
/// `cargo test -p hoard-core --test cleanup -- --ignored real_trash`
#[test]
#[ignore]
fn real_trash_moves_a_temp_file() {
    let fx = Fixture::new();
    fx.file("hoard-trash-test.txt", 64);
    let s = setup(fx);
    let id = s.id("hoard-trash-test.txt");
    let report = s
        .run_with(
            &[id],
            CleanupMode::Trash,
            &hoard_core::cleanup::SystemRemover,
            None,
        )
        .unwrap();
    assert!(
        matches!(outcome(&report, id), ItemOutcome::Removed),
        "{report:?}"
    );
    assert!(!s.fx.path("hoard-trash-test.txt").exists());
}

#[test]
fn partially_removed_folder_is_reported_and_marked_stale() {
    let fx = Fixture::new();
    fx.file("victim/a.bin", 1_000);
    fx.file("victim/locked/b.bin", 1_000);
    let s = setup(fx);
    let victim = s.id("victim");
    // Becomes unreadable after the scan, so removal cannot finish.
    let Some(guard) = s.fx.unreadable_dir("victim/locked") else {
        eprintln!("skipped: cannot create an unreadable directory as this user");
        return;
    };
    let report = s.run(&[victim], CleanupMode::Permanent);
    drop(guard);
    assert!(
        matches!(outcome(&report, victim), ItemOutcome::Partial(_)),
        "{report:?}"
    );
    assert!(
        !s.fx.path("victim/a.bin").exists(),
        "readable contents are removed"
    );
    assert!(
        s.fx.path("victim/locked/b.bin").exists(),
        "unreadable contents stay"
    );
    let tree = s.tree.read().unwrap();
    let flags = tree.get(victim).unwrap().flags;
    assert!(flags.contains(NodeFlags::STALE) && !flags.contains(NodeFlags::REMOVED));
    assert_eq!(s.log_lines()[0]["result"], "partial");
}

#[test]
fn ui_confirmation_phrase_matches_what_the_engine_requires() {
    let fx = Fixture::new();
    for i in 0..1001 {
        fx.file(&format!("many/f{i}.bin"), 3);
    }
    let s = setup(fx);
    let many = s.id("many");
    let shown = {
        let tree = s.tree.read().unwrap();
        hoard_core::cleanup::required_confirmation(&tree, &[many], CleanupMode::Permanent, &s.ctx)
    };
    let err = s
        .run_with(&[many], CleanupMode::Permanent, &FakeTrash::new(), None)
        .unwrap_err();
    let CleanupError::ConfirmationRequired { expected } = err else {
        panic!("{err:?}");
    };
    assert_eq!(shown, Some(expected));
}
