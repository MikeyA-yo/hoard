//! `hoard compare-mft <path>`: a hidden diagnostic that scans a folder twice,
//! by walking and from the NTFS MFT, and reports every difference. Used to
//! verify the MFT fast path on real volumes (needs an elevated prompt).

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Result};
use hoard_core::{format, MftMode, NodeFlags, NodeId, NodeKind, ScanOptions, ScanSource, Tree};

use crate::scan_with_progress;

const EXAMPLES: usize = 6;

#[derive(Default)]
struct Diff {
    count: u64,
    examples: Vec<String>,
}

#[derive(Default)]
struct Report {
    cats: BTreeMap<&'static str, Diff>,
    matched: u64,
}

impl Report {
    fn add(&mut self, cat: &'static str, what: impl FnOnce() -> String) {
        let d = self.cats.entry(cat).or_default();
        d.count += 1;
        if d.examples.len() < EXAMPLES {
            d.examples.push(what());
        }
    }
}

pub(crate) fn run(path: &Path, quiet: bool) -> Result<()> {
    let t = Instant::now();
    let mft_opts = ScanOptions {
        mft: MftMode::Force,
        ..ScanOptions::default()
    };
    let (mft, _) = scan_with_progress(path, mft_opts, quiet)?;
    let mft_secs = t.elapsed().as_secs_f64();
    if mft.source() != ScanSource::Mft {
        bail!(
            "the MFT could not be read for {} (run from an administrator prompt, on a local NTFS volume)",
            path.display()
        );
    }
    let t = Instant::now();
    let walk_opts = ScanOptions {
        mft: MftMode::Off,
        ..ScanOptions::default()
    };
    let (walk, _) = scan_with_progress(path, walk_opts, quiet)?;
    let walk_secs = t.elapsed().as_secs_f64();

    let mut report = Report::default();
    let mut stack = vec![(walk.root(), mft.root(), PathBuf::from(walk.root_path()))];
    while let Some((w, m, p)) = stack.pop() {
        let (Some(wn), Some(mn)) = (walk.get(w), mft.get(m)) else {
            continue;
        };
        report.matched += 1;
        compare_node(&walk, w, &mft, m, &p, &mut report);
        if wn.kind != NodeKind::Dir || mn.kind != NodeKind::Dir {
            continue;
        }
        let by_name = |t: &Tree, id: NodeId| -> HashMap<Box<OsStr>, NodeId> {
            t.get(id)
                .map(|n| {
                    n.children
                        .iter()
                        .filter_map(|c| Some((t.get(*c)?.name.clone(), *c)))
                        .collect()
                })
                .unwrap_or_default()
        };
        let wc = by_name(&walk, w);
        let mc = by_name(&mft, m);
        let walk_error = wn.flags.contains(NodeFlags::ERROR);
        for (name, wid) in &wc {
            match mc.get(name) {
                Some(mid) => stack.push((*wid, *mid, p.join(&**name))),
                None => report.add("only in walk", || p.join(&**name).display().to_string()),
            }
        }
        for name in mc.keys().filter(|n| !wc.contains_key(*n)) {
            let cat = if walk_error {
                "only in MFT (inside a folder the walk could not read)"
            } else {
                "only in MFT"
            };
            report.add(cat, || p.join(&**name).display().to_string());
        }
    }

    let total = |t: &Tree| {
        t.get(t.root()).map_or((0, 0, 0), |n| {
            (n.size_allocated, n.size_apparent, n.file_count)
        })
    };
    let (wa, wp, wf) = total(&walk);
    let (ma, mp, mf) = total(&mft);
    println!("{}", walk.root_path().display());
    println!(
        "  walk: {:>10} on disk, {:>10} apparent, {} files, {} folders, {} errors, {:.2}s",
        format::bytes(wa),
        format::bytes(wp),
        format::count(u64::from(wf)),
        format::count(walk.dir_count() as u64),
        walk.errors().len(),
        walk_secs
    );
    println!(
        "  MFT:  {:>10} on disk, {:>10} apparent, {} files, {} folders, {} errors, {:.2}s",
        format::bytes(ma),
        format::bytes(mp),
        format::count(u64::from(mf)),
        format::count(mft.dir_count() as u64),
        mft.errors().len(),
        mft_secs
    );
    println!(
        "  difference: {} bytes on disk ({:+.3}%), {} bytes apparent",
        ma as i128 - wa as i128,
        if wa == 0 {
            0.0
        } else {
            (ma as f64 - wa as f64) / wa as f64 * 100.0
        },
        mp as i128 - wp as i128
    );
    println!(
        "  entries matched by path: {}",
        format::count(report.matched)
    );
    if report.cats.is_empty() {
        println!("  no differences");
    }
    for (cat, d) in &report.cats {
        println!("  {cat}: {}", format::count(d.count));
        for e in &d.examples {
            println!("      {e}");
        }
    }
    for e in mft.errors().iter().take(3) {
        println!("  MFT error: {}", e.message);
    }
    Ok(())
}

/// Own (non-aggregated) allocated size of a node.
fn own_allocated(t: &Tree, id: NodeId) -> u64 {
    let Some(n) = t.get(id) else { return 0 };
    let children: u64 = n
        .children
        .iter()
        .filter_map(|c| t.get(*c))
        .map(|c| c.size_allocated)
        .sum();
    n.size_allocated.saturating_sub(children)
}

fn compare_node(walk: &Tree, w: NodeId, mft: &Tree, m: NodeId, p: &Path, r: &mut Report) {
    let (Some(a), Some(b)) = (walk.get(w), mft.get(m)) else {
        return;
    };
    let show = || p.display().to_string();
    if a.kind != b.kind {
        r.add("kind", || {
            format!("{}: walk {:?}, MFT {:?}", show(), a.kind, b.kind)
        });
        return;
    }
    if a.ino != b.ino {
        r.add("file id", || {
            format!("{}: walk {:#x}, MFT {:#x}", show(), a.ino, b.ino)
        });
    }
    if a.dev != b.dev {
        r.add("volume serial", || {
            format!("{}: walk {:#x}, MFT {:#x}", show(), a.dev, b.dev)
        });
    }
    if a.mtime != b.mtime && m != mft.root() {
        r.add("modified time", || {
            format!("{}: walk {}, MFT {}", show(), a.mtime, b.mtime)
        });
    }
    let linked =
        a.flags.contains(NodeFlags::HARDLINK_DUP) || b.flags.contains(NodeFlags::HARDLINK_DUP);
    match a.kind {
        NodeKind::File if !linked => {
            if a.size_apparent != b.size_apparent {
                r.add("file size", || {
                    format!(
                        "{}: walk {}, MFT {}",
                        show(),
                        a.size_apparent,
                        b.size_apparent
                    )
                });
            }
            if a.size_allocated != b.size_allocated {
                r.add("file allocated size", || {
                    format!(
                        "{}: walk {}, MFT {} (apparent {})",
                        show(),
                        a.size_allocated,
                        b.size_allocated,
                        a.size_apparent
                    )
                });
            }
        }
        NodeKind::Dir if m != mft.root() => {
            let (oa, ob) = (own_allocated(walk, w), own_allocated(mft, m));
            if oa != ob {
                r.add("folder index size", || {
                    format!("{}: walk {oa}, MFT {ob}", show())
                });
            }
        }
        _ => {}
    }
}
