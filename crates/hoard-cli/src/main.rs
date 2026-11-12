//! `hoard`: command line front end for `hoard-core`.

mod compare;

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use hoard_core::{
    format, scan, MftMode, NodeId, ScanEvent, ScanOptions, ScanSource, SizeMode, SortKey, Tree,
};
use serde::Serialize;

#[derive(Parser)]
#[command(name = "hoard", version, about = "See what's hoarding your disk")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a folder or drive and show what uses the most space.
    Scan(ScanArgs),
    /// Scan and list reclaimable space found by the built-in rules.
    Suggest(SuggestArgs),
    /// Diagnostic: scan by walking and from the NTFS MFT, and list every
    /// difference (Windows, administrator prompt).
    #[command(hide = true)]
    CompareMft {
        /// Folder or drive to compare.
        path: PathBuf,
        /// Hide the live progress line.
        #[arg(long, short)]
        quiet: bool,
    },
}

#[derive(Args)]
struct SuggestArgs {
    /// Folder or drive to scan.
    path: PathBuf,
    /// Items to show per rule.
    #[arg(long, default_value_t = 5)]
    top: usize,
    /// Print machine-readable JSON.
    #[arg(long)]
    json: bool,
    /// Descend into other filesystems / volumes.
    #[arg(long)]
    cross_fs: bool,
    /// Hide the live progress line.
    #[arg(long, short)]
    quiet: bool,
}

#[derive(Args)]
struct ScanArgs {
    /// Folder or drive to scan.
    path: PathBuf,
    /// Entries to show per level.
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// Levels below the root to show.
    #[arg(long, default_value_t = 1)]
    depth: usize,
    /// Report apparent (logical) size instead of allocated (on-disk) size.
    #[arg(long)]
    apparent: bool,
    /// Print machine-readable JSON instead of a table.
    #[arg(long)]
    json: bool,
    /// Descend into other filesystems / volumes.
    #[arg(long)]
    cross_fs: bool,
    /// Directory to list but not descend into (repeatable).
    #[arg(long, value_name = "PATH")]
    skip: Vec<PathBuf>,
    /// Worker threads (0 = automatic).
    #[arg(long, default_value_t = 0)]
    threads: usize,
    /// Number of errors to list (human output).
    #[arg(long, default_value_t = 5)]
    show_errors: usize,
    /// Hide the live progress line.
    #[arg(long, short)]
    quiet: bool,
    /// When to read the NTFS Master File Table directly instead of walking
    /// (Windows, administrator prompt). `auto` uses it for drive roots and
    /// the home folder or its parents.
    #[arg(long, value_enum, default_value_t = MftArg::Auto)]
    mft: MftArg,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum MftArg {
    Auto,
    Off,
    Force,
}

impl From<MftArg> for MftMode {
    fn from(a: MftArg) -> Self {
        match a {
            MftArg::Auto => MftMode::Auto,
            MftArg::Off => MftMode::Off,
            MftArg::Force => MftMode::Force,
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("HOARD_LOG"))
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Scan(args) => run_scan(args),
        Command::Suggest(args) => run_suggest(args),
        Command::CompareMft { path, quiet } => compare::run(&path, quiet),
    }
}

fn run_scan(args: ScanArgs) -> Result<()> {
    let opts = ScanOptions {
        cross_filesystems: args.cross_fs,
        apparent: args.apparent,
        skip: args.skip.clone(),
        threads: args.threads,
        mft: args.mft.into(),
    };
    let mode = if args.apparent {
        SizeMode::Apparent
    } else {
        SizeMode::Allocated
    };
    let (tree, elapsed) = scan_with_progress(&args.path, opts, args.quiet)?;

    if args.json {
        let report = JsonReport::build(&tree, mode, args.top, args.depth, elapsed.as_millis());
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
    } else {
        print_human(&tree, mode, &args, elapsed.as_secs_f64());
    }
    Ok(())
}

/// Scans `path`, showing a live progress line on stderr unless `quiet`.
fn scan_with_progress(
    path: &Path,
    opts: ScanOptions,
    quiet: bool,
) -> Result<(Tree, std::time::Duration)> {
    let (tx, rx) = crossbeam_channel::bounded::<ScanEvent>(64);
    let show_progress = !quiet && std::io::stderr().is_terminal();
    let printer = thread::spawn(move || {
        let mut printed = false;
        for event in rx {
            if let (
                true,
                ScanEvent::Progress {
                    files,
                    bytes,
                    current_path,
                    errors,
                    ..
                },
            ) = (show_progress, event)
            {
                let line = format!(
                    "{} files, {}, {} errors  {}",
                    format::count(files),
                    format::bytes(bytes),
                    errors,
                    current_path.display()
                );
                let mut err = std::io::stderr().lock();
                let _ = write!(err, "\r\x1b[2K{}", truncate(&line, 120));
                let _ = err.flush();
                printed = true;
            }
        }
        if printed {
            let _ = write!(std::io::stderr().lock(), "\r\x1b[2K");
        }
    });

    let started = Instant::now();
    let tree = scan(path, opts, tx, Arc::new(AtomicBool::new(false)))
        .with_context(|| format!("scanning {}", path.display()))?;
    let elapsed = started.elapsed();
    let _ = printer.join();

    Ok((tree, elapsed))
}

fn run_suggest(args: SuggestArgs) -> Result<()> {
    use hoard_core::rules::{suggest, MatchContext, RuleSet, Safety};

    let opts = ScanOptions {
        cross_filesystems: args.cross_fs,
        ..ScanOptions::default()
    };
    let (tree, _) = scan_with_progress(&args.path, opts, args.quiet)?;
    let rules = RuleSet::builtin().context("loading built-in rules")?;
    let groups = suggest(&tree, &rules, &MatchContext::current());

    if args.json {
        #[derive(Serialize)]
        struct JsonItem {
            path: PathBuf,
            size_allocated: u64,
            size_apparent: u64,
            newest_mtime: i64,
        }
        #[derive(Serialize)]
        struct JsonGroup<'a> {
            rule: &'a hoard_core::rules::Rule,
            total_allocated: u64,
            total_apparent: u64,
            count: usize,
            items: Vec<JsonItem>,
        }
        let out: Vec<JsonGroup<'_>> = groups
            .iter()
            .map(|g| JsonGroup {
                rule: g.rule,
                total_allocated: g.total_allocated,
                total_apparent: g.total_apparent,
                count: g.items.len(),
                items: g
                    .items
                    .iter()
                    .take(args.top)
                    .map(|i| JsonItem {
                        path: tree.path(i.node).unwrap_or_default(),
                        size_allocated: i.size_allocated,
                        size_apparent: i.size_apparent,
                        newest_mtime: i.newest_mtime,
                    })
                    .collect(),
            })
            .collect();
        serde_json::to_writer_pretty(std::io::stdout().lock(), &out)?;
        println!();
        return Ok(());
    }

    let total = |s: Safety| -> u64 {
        groups
            .iter()
            .filter(|g| g.rule.safety == s && (g.rule.addable || s != Safety::Safe))
            .map(|g| g.total_allocated)
            .sum()
    };
    println!(
        "{}: Safe {} (removable here) · Review {} · Caution {} (on disk)",
        tree.root_path().display(),
        format::bytes(total(Safety::Safe)),
        format::bytes(total(Safety::Review)),
        format::bytes(total(Safety::Caution)),
    );
    if groups.is_empty() {
        println!("\nNothing matched the built-in rules here.");
    }
    for g in &groups {
        let label = match g.rule.safety {
            Safety::Safe => "SAFE",
            Safety::Review => "REVIEW",
            Safety::Caution => "CAUTION",
        };
        println!();
        println!(
            "[{label}] {}  {}  ({} item{}){}",
            g.rule.name,
            format::bytes(g.total_allocated),
            format::count(g.items.len() as u64),
            if g.items.len() == 1 { "" } else { "s" },
            if g.rule.addable {
                ""
            } else {
                "  (info only, not removable here)"
            }
        );
        println!("    {}", g.rule.explain);
        if let Some(r) = &g.rule.regenerate {
            println!("    Comes back / instead: {r}");
        }
        for i in g.items.iter().take(args.top) {
            let path = tree.path(i.node).unwrap_or_default();
            println!(
                "    {:>10}  {}",
                format::bytes(i.size_allocated),
                path.display()
            );
        }
        if g.items.len() > args.top {
            println!("    ... and {} more", g.items.len() - args.top);
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let tail: String = s
        .chars()
        .rev()
        .take(max - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

fn print_human(tree: &Tree, mode: SizeMode, args: &ScanArgs, secs: f64) {
    let Some(root) = tree.get(tree.root()) else {
        return;
    };
    let mode_label = match mode {
        SizeMode::Allocated => "on disk",
        SizeMode::Apparent => "apparent",
    };
    println!(
        "{}  {} {}, {} files, {} folders, {} errors, {:.2}s{}{}",
        tree.root_path().display(),
        format::bytes(root.size(mode)),
        mode_label,
        format::count(u64::from(root.file_count)),
        format::count(tree.dir_count() as u64),
        format::count(tree.errors().len() as u64),
        secs,
        if tree.source() == ScanSource::Mft {
            "  (read from the NTFS MFT)"
        } else {
            ""
        },
        if tree.is_complete() {
            ""
        } else {
            "  (INCOMPLETE)"
        }
    );
    println!();
    print_level(tree, tree.root(), mode, args.top, args.depth, 0);

    if !tree.errors().is_empty() && args.show_errors > 0 {
        println!();
        println!("Errors (sizes below these paths are unknown):");
        for e in tree.errors().iter().take(args.show_errors) {
            println!("  {}: {}", e.path.display(), e.message);
        }
        if tree.errors().len() > args.show_errors {
            println!(
                "  ... and {} more (use --json for all)",
                tree.errors().len() - args.show_errors
            );
        }
    }
}

fn print_level(tree: &Tree, id: NodeId, mode: SizeMode, top: usize, depth: usize, level: usize) {
    if level >= depth {
        return;
    }
    let Some(parent) = tree.get(id) else { return };
    let parent_size = parent.size(mode).max(1);
    let children = tree.sorted_children(id, SortKey::Size, mode);
    let indent = "  ".repeat(level);
    for &child in children.iter().take(top) {
        let Some(n) = tree.get(child) else { continue };
        let pct = n.size(mode) as f64 * 100.0 / parent_size as f64;
        let suffix = match n.kind {
            hoard_core::NodeKind::Dir => std::path::MAIN_SEPARATOR_STR,
            hoard_core::NodeKind::Symlink => " ->",
            _ => "",
        };
        let flags = if n.flags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", n.flags.names().join(", "))
        };
        println!(
            "{:>10}  {:>5.1}%  {indent}{}{suffix}{flags}",
            format::bytes(n.size(mode)),
            pct,
            n.display_name()
        );
        if n.is_dir() {
            print_level(tree, child, mode, top, depth, level + 1);
        }
    }
    if children.len() > top {
        let rest: u64 = children[top..]
            .iter()
            .filter_map(|c| tree.get(*c))
            .map(|n| n.size(mode))
            .sum();
        println!(
            "{:>10}  {:>5.1}%  {indent}({} smaller items)",
            format::bytes(rest),
            rest as f64 * 100.0 / parent_size as f64,
            format::count((children.len() - top) as u64)
        );
    }
}

#[derive(Serialize)]
struct JsonReport {
    root: PathBuf,
    complete: bool,
    source: ScanSource,
    size_mode: SizeMode,
    elapsed_ms: u128,
    size_allocated: u64,
    size_apparent: u64,
    files: u32,
    dirs: usize,
    error_count: usize,
    errors: Vec<hoard_core::ScanError>,
    entries: Vec<JsonEntry>,
    rest: Option<JsonRest>,
}

#[derive(Serialize)]
struct JsonEntry {
    name: String,
    path: PathBuf,
    kind: hoard_core::NodeKind,
    size_allocated: u64,
    size_apparent: u64,
    file_count: u32,
    mtime: i64,
    flags: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<JsonEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rest: Option<JsonRest>,
}

/// Summary of children beyond `--top`.
#[derive(Serialize)]
struct JsonRest {
    count: usize,
    size_allocated: u64,
    size_apparent: u64,
}

/// Hard cap on errors in JSON output, so a scan of `/` cannot emit megabytes.
const JSON_MAX_ERRORS: usize = 10_000;

impl JsonReport {
    fn build(tree: &Tree, mode: SizeMode, top: usize, depth: usize, elapsed_ms: u128) -> Self {
        let root = tree.get(tree.root());
        let (entries, rest) = json_children(tree, tree.root(), mode, top, depth, 0);
        JsonReport {
            root: tree.root_path().to_path_buf(),
            complete: tree.is_complete(),
            source: tree.source(),
            size_mode: mode,
            elapsed_ms,
            size_allocated: root.map_or(0, |n| n.size_allocated),
            size_apparent: root.map_or(0, |n| n.size_apparent),
            files: root.map_or(0, |n| n.file_count),
            dirs: tree.dir_count(),
            error_count: tree.errors().len(),
            errors: tree
                .errors()
                .iter()
                .take(JSON_MAX_ERRORS)
                .cloned()
                .collect(),
            entries,
            rest,
        }
    }
}

fn json_children(
    tree: &Tree,
    id: NodeId,
    mode: SizeMode,
    top: usize,
    depth: usize,
    level: usize,
) -> (Vec<JsonEntry>, Option<JsonRest>) {
    if level >= depth {
        return (Vec::new(), None);
    }
    let children = tree.sorted_children(id, SortKey::Size, mode);
    let entries = children
        .iter()
        .take(top)
        .filter_map(|&c| {
            let n = tree.get(c)?;
            let (kids, rest) = json_children(tree, c, mode, top, depth, level + 1);
            Some(JsonEntry {
                name: n.display_name(),
                path: tree
                    .path(c)
                    .unwrap_or_else(|| Path::new(&*n.name).to_path_buf()),
                kind: n.kind,
                size_allocated: n.size_allocated,
                size_apparent: n.size_apparent,
                file_count: n.file_count,
                mtime: n.mtime,
                flags: n.flags.names(),
                children: kids,
                rest,
            })
        })
        .collect();
    let rest = (children.len() > top).then(|| {
        let others = children[top..].iter().filter_map(|c| tree.get(*c));
        let (a, b) = others.fold((0u64, 0u64), |(a, b), n| {
            (a + n.size_allocated, b + n.size_apparent)
        });
        JsonRest {
            count: children.len() - top,
            size_allocated: a,
            size_apparent: b,
        }
    });
    (entries, rest)
}
