# Hoard

Cross-platform disk usage visualizer and cleaner, written in Rust. See
[docs/PRD.md](docs/PRD.md) for what and why, and [CLAUDE.md](CLAUDE.md) for
how the code is organised.

## Status

| Milestone | State |
|-----------|-------|
| M0 Core + CLI | Done |
| M1 Desktop viewer | Done (Windows verified; macOS/Linux untested) |
| M2 Cleanup | Done (Windows verified; real Recycle Bin test is manual) |
| M3 Lenses + suggestions | Done (Windows verified) |
| M4 Apps, duplicates, history | Not started |
| M5 Platform depth | Not started |

## CLI

```bash
cargo run --release -p hoard-cli -- scan ~ --top 20
cargo run --release -p hoard-cli -- scan ~ --depth 2 --top 10
cargo run --release -p hoard-cli -- scan ~ --json > scan.json
```

```bash
cargo run --release -p hoard-cli -- suggest ~        # reclaimable space by rule
```

Options: `--top N`, `--depth N`, `--apparent`, `--json`, `--cross-fs`,
`--skip PATH` (repeatable), `--threads N`, `--show-errors N`, `-q`.

Sizes default to allocated (on-disk) bytes, shown with binary multiples
(`1.0 KB` = 1024 bytes). Hard links are counted once, symlinks and junctions
are never followed, and unreadable folders are listed as errors without
stopping the scan.

## Desktop app

```bash
cd apps/desktop
pnpm install
pnpm tauri dev      # first build compiles ~400 crates, later builds are incremental
pnpm test           # vitest: formatting and treemap layout helpers
pnpm lint           # eslint + tsc
```

Windows builds work with the MSYS2 MinGW (`x86_64-pc-windows-gnu`) toolchain;
Visual Studio is not required. GNU builds place `WebView2Loader.dll` next to
the exe, and it must ship with the app.

What M1 covers: drive and folder picker, live scan with progress and a
partial treemap, squarified canvas treemap (color by type or age) beside a
sortable list, drill-down by double-click, Enter or breadcrumbs, Backspace to
go up, largest files and folders, an errors panel, and Show in Explorer /
Open / Copy path from the right-click menu.

IPC commands beyond the CLAUDE.md table: `home_dir`, `get_summary`,
`get_errors`, `reveal_node`, `open_node` (both take a node id, never a path),
and an explicit `mode` (allocated or apparent) on the query commands.

## Cleanup (M2)

Nothing is deleted except from the Cleanup Tray (right-click, or Space on a
selected item). The default is Move to Recycle Bin / Trash; permanent delete
must be enabled per run and asks for the typed total above 10 GB or 1,000
files. Every item is re-checked just before removal: it must still be the
same file or folder as when scanned (device + inode), not on the protected
list, not the scanned folder or one of its parents, and still inside the
scanned folder. Links are removed as links. Anything that fails a check is
skipped and reported, never retried another way. Each item gets a line in
`actions.jsonl` in the app data folder, and the report shows estimated and
measured freed space (Recycle Bin moves free space only when it is emptied).

The real Recycle Bin test is manual:

```bash
cargo test -p hoard-core --test cleanup -- --ignored real_trash
```

## Suggestions (M3)

Rules live in [rules/default.toml](rules/default.toml): package caches, dev
build output, browser and app caches, old temp files, and Downloads. Each is
labelled Safe, Review or Caution with what it is and how it comes back.
"Add all Safe items" fills the tray; Review items are added one at a time;
information-only rules (Docker/WSL disks, the Go module cache) can never be
added. Docker and WSL virtual disks, and folders containing them, are also on
the protected list. Every rule has a match and near-miss test in
`crates/hoard-core/tests/rules.rs`.

Lenses (the view switcher in the top bar): **Map** (folders), **Suggestions**
(filterable by dev projects, caches and system, downloads), **Projects**
(build and dependency folders per project, with last source-file activity;
sort by least recently active to find abandoned projects) and **File types**
(totals per category, then the largest files of one). The Tray shows how much
the Recycle Bin is holding, since that space is only freed once it is
emptied.

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo bench -p hoard-core
```
