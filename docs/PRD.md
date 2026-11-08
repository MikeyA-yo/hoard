# Hoard: Product Requirements Document

**Status:** Draft v0.1
**Owner:** Ay
**Last updated:** 8 Nov 2026
**Working name:** Hoard (shows you what's hoarding your disk). Rename freely.

---

## 1. Summary

Hoard is a cross-platform desktop app written in Rust that scans a drive or folder, shows exactly what is using space (folders, files, apps, file types, caches, dev artifacts), and lets you clean up safely from the same screen.

The bar: WizTree/DaisyDisk speed and clarity, plus two things most tools skip. First, it attributes scattered data back to its owner (an app's caches, containers and support files count toward that app, not just "Library"). Second, it understands developer junk (`node_modules`, `target/`, DerivedData, package caches) and tells you what's safe to delete and how it regenerates.

## 2. Problem

- OS storage panels lump everything into vague buckets like "System Data" or "Other" with no drill-down.
- Existing visualizers (WinDirStat, WizTree, DaisyDisk, GrandPerspective, ncdu, dust, dua) show folder sizes well but are platform-locked, have no idea which files belong to which app, give no guidance on what's safe to remove, and have no memory of previous scans, so you can't answer "what grew since last week?"
- Developers on 256 to 512 GB laptops quietly accumulate tens to hundreds of GB in build outputs, dependency folders, simulators, Docker images and package caches spread across dozens of projects.

## 3. Goals

| ID | Goal |
|----|------|
| G1 | Scan fast enough that rescanning is never a chore (home dir with ~1M files in under 15s on NVMe). |
| G2 | Make the biggest consumers obvious within 2 seconds of looking at the screen. |
| G3 | Attribute space to meaningful owners: folders, apps, file types, projects, caches. |
| G4 | Clean up from inside the app with safety that makes accidental data loss practically impossible. |
| G5 | Track change over time so growth is visible, not a surprise. |

### Non-goals (v1)

- Cloud storage management (iCloud, OneDrive, Google Drive placeholders are shown, not managed).
- Background daemon or automatic cleaning. Hoard only acts when you click.
- Registry cleaning, "PC optimizer" features, RAM cleaning.
- Secure erase / shredding.
- Optimizing for network drives (they can be scanned, just not fast).

## 4. Users

**Primary:** a developer (me) on a laptop with a constrained SSD, many side projects, multiple language toolchains, and no patience for vague storage panels.

**Secondary:** power users who want a better DaisyDisk/WizTree that works the same on every OS.

## 5. User stories

1. As a user, I pick a drive or folder and see a live scan with progress, and I can cancel it.
2. As a user, I see a treemap of my disk and click into any block to drill down, with breadcrumbs to go back up.
3. As a user, I see a sorted list of the largest folders and files under the current node.
4. As a user, I switch to an Apps view and see each installed app's total footprint (binary + support data + caches), not just the `.app` or `Program Files` folder.
5. As a developer, I see all `node_modules`, `target/`, `.venv` etc. across my projects, grouped by project, with when I last touched each project.
6. As a user, I see "Suggestions": a list of reclaimable space, each item labeled Safe / Review / Caution with a one-line explanation and how it regenerates.
7. As a user, I add items to a Cleanup Tray from any view, review the total, and move them to Trash in one action.
8. As a user, after cleanup I see how much space was actually freed (measured), not just estimated.
9. As a user, I compare today's scan with last week's and see what grew, shrank, appeared or disappeared.
10. As a user, I can find duplicate files and keep one copy.
11. As a developer, I run `hoard scan ~ --top 20` from a terminal or SSH session and get the same numbers as the GUI.

## 6. Functional requirements

Priority: **P0** = required for v1, **P1** = v1 if time allows, **P2** = later.

### 6.1 Scanning

| ID | Requirement | Pri |
|----|-------------|-----|
| S1 | List mounted disks with capacity, used and free space; allow scanning a whole disk, the home folder, or any folder. | P0 |
| S2 | Parallel directory walk across all cores, results streamed to the UI while scanning (the treemap fills in progressively). | P0 |
| S3 | Live progress: files scanned, bytes counted, current path, elapsed time, errors. Cancellable at any time. | P0 |
| S4 | Size accounting defaults to **allocated (on-disk) size**, with a toggle for apparent size. | P0 |
| S5 | Hard links counted once (keyed by device + inode / file ID). | P0 |
| S6 | Symlinks are never followed. Filesystem boundaries are not crossed by default (option to allow). Virtual filesystems (`/proc`, `/sys`, `/dev`) are always skipped. | P0 |
| S7 | Permission errors never abort a scan. They are counted, listed, and the affected size is reported as unknown. | P0 |
| S8 | "Unaccounted space" figure: disk used minus scanned total, with an explanation panel (permissions, APFS snapshots, purgeable space, system-reserved). | P1 |
| S9 | Rescan a single subtree without rescanning everything. | P1 |
| S10 | Windows fast path: read the NTFS MFT directly when running as admin (WizTree-style), falling back to normal walk otherwise. | P2 |

### 6.2 Visualization

| ID | Requirement | Pri |
|----|-------------|-----|
| V1 | Squarified treemap rendered on canvas, colored by file category (default) or by age. | P0 |
| V2 | List view beside the treemap: sortable by size, item count, last modified; shows % of parent. | P0 |
| V3 | Drill down by click, up by breadcrumb / Backspace. Hover shows full path, size, count, modified date. | P0 |
| V4 | Global "Largest files" and "Largest folders" panels for the current scan. | P0 |
| V5 | Search and filters: name/glob, extension, size greater than, modified before. | P1 |
| V6 | Sunburst view as an alternative to the treemap. | P2 |
| V7 | Context actions everywhere: Reveal in Finder/Explorer, Open, Copy path, Add to Cleanup Tray. | P0 |

### 6.3 Lenses (alternative groupings of the same scan)

| ID | Lens | What it shows | Pri |
|----|------|---------------|-----|
| L1 | Folders | Default hierarchical view. | P0 |
| L2 | File types | Totals by category: video, images, audio, archives, disk images, documents, code, other. Click a category to list its largest files. | P0 |
| L3 | Dev artifacts | Detected build/dependency dirs grouped by project root, with project last-activity date (newest mtime of source files, excluding the artifact dir). | P0 |
| L4 | Caches & system | Package manager caches, browser caches, logs, Trash/Recycle Bin, temp dirs. | P1 |
| L5 | Apps | Installed apps with binary size + data size + cache size, attribution confidence shown (exact / likely). | P1 |
| L6 | Duplicates | Groups of identical files. Pipeline: same size, then first/last 64 KB hash, then full BLAKE3. | P1 |
| L7 | Old & large | Files over 500 MB not modified in 180+ days. (mtime only; atime is unreliable and often disabled.) | P1 |

**App attribution per platform (L5):**

| OS | App source | Associated data |
|----|-----------|-----------------|
| macOS | `/Applications`, `~/Applications`; read `CFBundleIdentifier` from `Info.plist` | Match bundle ID and app name under `~/Library/{Application Support, Caches, Containers, Group Containers, Preferences, Logs, Saved Application State, HTTPStorages, WebKit}` |
| Windows | Uninstall registry keys (HKLM + HKCU) for name, publisher, `InstallLocation`, `EstimatedSize` | `%APPDATA%`, `%LOCALAPPDATA%`, `%PROGRAMDATA%` matched by publisher/app name (marked "likely") |
| Linux | `.desktop` files, Flatpak (`/var/lib/flatpak`, `~/.var/app`), Snap (`/snap`, `~/snap`) | `~/.config`, `~/.cache`, `~/.local/share` matched by app name |

### 6.4 Cleanup

| ID | Requirement | Pri |
|----|-------------|-----|
| C1 | **Cleanup Tray**: a staging area. Items can be added from any view; it shows count and estimated reclaimable total; items can be removed before execution. Nothing is deleted except from the tray. | P0 |
| C2 | Default action is **Move to Trash / Recycle Bin**. | P0 |
| C3 | Permanent delete is a separate, explicitly enabled mode. It requires typing the total size (e.g. `42.1 GB`) to confirm when over 10 GB or over 1,000 items. | P0 |
| C4 | Pre-execution validation per item (see section 6.7). Items that fail validation are skipped and reported, never "best-efforted". | P0 |
| C5 | Post-cleanup report: per-item result, estimated vs **measured** freed space (free-space delta of the volume), and any failures. | P0 |
| C6 | Append-only action log (JSONL) in the app data dir: timestamp, path, size, mode, result. | P0 |
| C7 | Tree updates in place after cleanup without a full rescan. | P0 |
| C8 | Dev artifact cleanup shows the regenerate command (e.g. `pnpm install`, `cargo build`). | P1 |
| C9 | Warn when deleting data that belongs to a currently running app. | P2 |
| C10 | App uninstall: macOS moves the `.app` plus attributed data to Trash; Windows launches the registered uninstaller, then offers leftover data. | P2 |

### 6.5 Suggestions engine

| ID | Requirement | Pri |
|----|-------------|-----|
| R1 | Rule-based detection. Each rule has: id, name, matcher, safety level (Safe / Review / Caution), explanation, regenerate hint. | P1 |
| R2 | Rules live in a TOML file shipped with the app, so adding one doesn't require touching engine code. | P1 |
| R3 | Suggestions page sorted by reclaimable size, with one-click "Add all Safe items to tray". Review and Caution items are never bulk-added. | P1 |

**Built-in rules (initial set):**

| Rule | Detection | Safety | Regenerates via |
|------|-----------|--------|-----------------|
| `node_modules` | dir named `node_modules` with sibling `package.json` | Safe | `npm/pnpm/yarn install` |
| Rust `target/` | `target/` with sibling `Cargo.toml` | Safe | `cargo build` |
| Python venv | dir containing `pyvenv.cfg` | Review | recreate venv + install |
| `__pycache__`, `.pytest_cache` | by name | Safe | automatic |
| JS framework caches | `.next`, `.turbo`, `.parcel-cache`, `.svelte-kit`, `.nuxt` | Safe | next build/dev |
| Xcode DerivedData | `~/Library/Developer/Xcode/DerivedData` | Safe | next build |
| iOS simulators / DeviceSupport | `~/Library/Developer/CoreSimulator`, `iOS DeviceSupport` | Review | re-download |
| Package caches | `~/.npm/_cacache`, pnpm store, yarn cache, `~/.cargo/registry`, pip cache, `~/.gradle/caches` | Safe | re-download on demand |
| Go module cache | `~/go/pkg/mod` | Review | files are read-only; use `go clean -modcache` rather than raw delete |
| Docker | detected via `docker system df` | Review | **never touch `Docker.raw` / VHDX directly**; offer `docker system prune` (P2) |
| Old installers | `.dmg .pkg .exe .msi .iso` in Downloads | Review | re-download |
| Old downloads | Downloads items untouched 90+ days | Review | n/a |
| Logs | user-owned log dirs | Safe | automatic |
| Trash / Recycle Bin | platform trash | Caution (permanent) | n/a |

### 6.6 Snapshots and history

| ID | Requirement | Pri |
|----|-------------|-----|
| H1 | Save a compact snapshot after each scan: every directory down to depth 6 plus any directory over 50 MB at any depth. | P1 |
| H2 | Diff any two snapshots of the same root: grew, shrank, new, removed, sorted by absolute change. | P1 |
| H3 | Home screen shows "Since last scan: +12.4 GB" with the top 5 growers. | P1 |

### 6.7 Safety model (applies to every deletion)

Before deleting any item, the engine must:

1. Confirm the path was produced by the current scan (it's a node in the tree), not typed or constructed.
2. Reject it if it matches the protected list: filesystem roots, OS dirs (`/System`, `/usr`, `/bin`, `C:\Windows`, `C:\Program Files` root itself, etc.), the user's home dir itself, top-level user folders themselves (Documents, Desktop, Pictures), and any ancestor of the scan root.
3. Re-stat with `symlink_metadata` and compare device + inode (or Windows file ID) to what was scanned. Mismatch means the path changed underneath us: skip.
4. Never follow a symlink. A symlink is deleted as a link, never its target.
5. Execute via Trash (default) or permanent remove, log the result, then update the tree.

### 6.8 CLI

| ID | Requirement | Pri |
|----|-------------|-----|
| X1 | `hoard scan <path> [--top N] [--apparent] [--json] [--depth N]` | P0 |
| X2 | `hoard suggest <path> [--json]` | P1 |
| X3 | `hoard clean --from <json> [--trash\|--permanent] [--dry-run]`, dry-run is the default. | P2 |

## 7. Platform notes

| | macOS | Windows | Linux |
|---|-------|---------|-------|
| Permissions | Prompt for Full Disk Access with a guided screen; scan works without it but reports unreadable areas. | Works as normal user; admin unlocks MFT fast path and system dirs. | Normal user; root not required. |
| Size quirks | APFS clones and sparse files mean freed space can be less than estimated. Local snapshots and "purgeable" space explain unaccounted gaps. | Use `GetCompressedFileSizeW` for on-disk size. Support long paths (`\\?\` prefix). Cloud placeholder files report size but occupy little. | `st_blocks * 512` for allocated size. Skip pseudo filesystems. |
| Trash | `trash` crate (Finder trash) | Recycle Bin via `trash` crate | freedesktop trash spec via `trash` crate |

## 8. Non-functional requirements

| Area | Target |
|------|--------|
| Scan speed | ~1M files under 15s on NVMe, warm cache. Ideally within 1.5x of `dua`/`dust` on the same tree. |
| Memory | Under 300 MB for a 2M-entry tree. |
| UI | 60 fps pan/hover on the treemap; IPC calls return in under 50 ms for any view. Never ship the whole tree to the frontend. |
| Startup | App window interactive in under 1s. |
| Binary | Installer under 20 MB. |
| Accuracy | Scanned total within 2% of `du -s` (allocated) on the same tree. |
| Privacy | No telemetry, no network calls except optional update checks. Nothing leaves the machine. |
| Accessibility | Full keyboard navigation of list view and treemap; sizes always shown as text, never color-only. |

## 9. Technical approach (summary)

- **Shape:** Cargo workspace with a pure Rust core library (`hoard-core`), a CLI (`hoard-cli`), and a Tauri 2 desktop app with a React + TypeScript frontend.
- **Why Tauri over egui/iced:** treemap/sunburst interaction, polish and theming are much faster to build with web tech I already know, while every byte of scanning, accounting and deletion stays in Rust. Tauri also keeps the binary small. The core is UI-agnostic, so a TUI or native UI can be added later without touching it.
- **Data model:** arena-allocated tree (`Vec<Node>` indexed by `u32` ids), sizes aggregated bottom-up after the walk; the frontend requests views (children page, treemap slice, search results), never the tree.
- **Scanner:** `jwalk` (rayon-based parallel walk) streaming batches through a channel; progress events throttled to ~10/s.

Full engineering detail lives in `CLAUDE.md`.

## 10. Milestones

| # | Milestone | Exit criteria |
|---|-----------|---------------|
| M0 | Core + CLI | `hoard scan ~ --top 20` correct vs `du`, hardlinks/symlinks/permission errors covered by tests, benchmark baseline recorded. |
| M1 | Desktop viewer | Disk picker, live scan, treemap + list, drill-down, largest files/folders, reveal/open. |
| M2 | Cleanup | Tray, Trash + permanent modes, full safety model (6.7) with tests, log, measured-freed report. |
| M3 | Lenses + suggestions | File types, dev artifacts, caches lenses; rules engine with built-in rules. |
| M4 | Apps + duplicates + history | Apps lens for current OS first, duplicates, snapshots and diff. |
| M5 | Platform depth | Other two OSes for Apps lens, Windows MFT fast path, Docker integration, app uninstall. |

## 11. Success metrics

- First real run on my own machine reclaims 20+ GB that I didn't know about.
- Home dir rescan under 15s, so I actually use it weekly.
- Zero unintended deletions: every deletion traceable in the log to a tray action I confirmed.
- Reported totals within 2% of `du` across test trees on all three OSes.

## 12. Risks and open questions

| Item | Notes |
|------|-------|
| Estimated vs actual freed space | APFS clones, hardlinks outside the scan root, and snapshots make estimates optimistic. Mitigation: always show measured free-space delta after cleanup and label estimates as such. |
| App attribution on Windows | AppData folder names don't reliably map to apps. Mitigation: show confidence level; never bulk-add "likely" matches to the tray. |
| macOS Full Disk Access UX | Users may deny it. The app must stay useful without it and explain exactly what's hidden. |
| Code signing / notarization | Needed for a smooth install on macOS and to avoid SmartScreen on Windows. Decide before any public release. |
| Open: distribution | Personal tool only, or open source on GitHub with releases? Affects signing priority. |
| Open: TUI | Is a `ncdu`-style TUI worth building on the same core after M2? |
| Open: name | "Hoard" is a working name. |
