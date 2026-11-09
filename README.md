# Hoard

Cross-platform disk usage visualizer and cleaner, written in Rust. See
[docs/PRD.md](docs/PRD.md) for what and why, and [CLAUDE.md](CLAUDE.md) for
how the code is organised.

## Status

| Milestone | State |
|-----------|-------|
| M0 Core + CLI | Done |
| M1 Desktop viewer | Done (Windows verified; macOS/Linux untested) |
| M2 Cleanup | Not started |
| M3 Lenses + suggestions | Not started |
| M4 Apps, duplicates, history | Not started |
| M5 Platform depth | Not started |

## CLI

```bash
cargo run --release -p hoard-cli -- scan ~ --top 20
cargo run --release -p hoard-cli -- scan ~ --depth 2 --top 10
cargo run --release -p hoard-cli -- scan ~ --json > scan.json
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

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo bench -p hoard-core
```
