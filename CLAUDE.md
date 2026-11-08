# CLAUDE.md

Instructions for working on **Hoard**, a cross-platform disk usage visualizer and cleaner written in Rust. `docs/PRD.md` is the source of truth for *what* and *why*. This file covers *how*.

## Golden rules (non-negotiable)

1. **Deletion code is the most dangerous code in this repo.** Any change under `crates/hoard-core/src/cleanup/` must come with tests written first, and must preserve every check in the Safety Model (PRD section 6.7).
2. **Never touch real user paths in tests.** All filesystem tests use `tempfile::TempDir`. No test may delete, move or trash anything outside a temp dir. Trash tests are `#[ignore]` by default and run manually.
3. **Never follow symlinks**, in scanning or deleting. Use `symlink_metadata`, not `metadata`.
4. **The scanner never panics on filesystem errors.** Every IO error becomes a recorded `ScanError` and the walk continues.
5. **Never send the whole tree over IPC.** The frontend asks for views (a page of children, a treemap slice, search results). If a command can return more than ~5,000 items, it must paginate or truncate.
6. **No `unwrap()` / `expect()` outside tests** except on invariants that are truly impossible, with a comment saying why.
7. Work **milestone by milestone** (PRD section 10). Don't build M(n+1) features until M(n) exit criteria pass.

## Repo layout

```
hoard/
├── Cargo.toml                 # workspace
├── crates/
│   ├── hoard-core/            # all logic, no UI, no Tauri dependency
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── scan/          # walker, progress, cancellation
│   │       ├── tree.rs        # arena tree, aggregation, queries
│   │       ├── size.rs        # allocated vs apparent size per platform
│   │       ├── classify/      # file type categories, dev artifact detection
│   │       ├── rules/         # suggestion engine, loads rules/default.toml
│   │       ├── apps/          # app discovery + attribution, per OS
│   │       ├── dupes.rs       # duplicate finder
│   │       ├── snapshot/      # save/load/diff
│   │       ├── cleanup/       # tray, validation, trash/delete, action log
│   │       └── platform/      # macos.rs, windows.rs, linux.rs behind cfg
│   └── hoard-cli/             # thin clap wrapper over hoard-core
├── apps/desktop/
│   ├── src-tauri/             # Tauri commands + app state ONLY, no business logic
│   └── src/                   # React + TypeScript + Vite frontend
├── rules/default.toml         # built-in suggestion rules
└── docs/PRD.md
```

If logic is creeping into `src-tauri/`, move it to `hoard-core`.

## Stack

| Concern | Choice |
|---------|--------|
| Parallel walk | `jwalk` (rayon under the hood) |
| Parallelism | `rayon`, `crossbeam-channel` |
| Errors | `thiserror` in core, `anyhow` in CLI and Tauri layer |
| Logging | `tracing`, `tracing-subscriber` |
| CLI | `clap` (derive) |
| Disks | `sysinfo` |
| Trash | `trash` |
| Hashing | `blake3` |
| Serialization | `serde`, `serde_json`; `postcard` + `zstd` for snapshots |
| Config/rules | `toml` |
| Platform dirs | `dirs` |
| macOS plist | `plist` |
| Windows | `windows` crate (file IDs, `GetCompressedFileSizeW`), `winreg` |
| Tests | `tempfile`, `assert_fs`, `criterion` for benches |
| Desktop | Tauri 2, React 18, TypeScript (strict), Vite, `d3-hierarchy` for treemap layout, canvas rendering, Zustand for state |

Prefer these. If you need a new dependency, say why in the commit message and pick a maintained crate.

## Commands

```bash
# core + cli
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p hoard-cli -- scan ~ --top 20
cargo bench -p hoard-core

# desktop
cd apps/desktop
pnpm install
pnpm tauri dev
pnpm test          # vitest
pnpm lint
```

Before considering any task done: `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` all pass.

## Core data model

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub u32);

pub struct Node {
    pub name: Box<str>,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,     // empty for files; optimize later if memory demands
    pub kind: NodeKind,            // File | Dir | Symlink | Other
    pub size_allocated: u64,       // aggregated for dirs
    pub size_apparent: u64,        // aggregated for dirs
    pub file_count: u32,           // aggregated for dirs
    pub mtime: i64,                // unix seconds
    pub dev: u64,
    pub ino: u64,                  // or Windows file ID
    pub flags: NodeFlags,          // ERROR, HARDLINK_DUP, PROTECTED, etc.
}

pub struct Tree {
    nodes: Vec<Node>,
    root: NodeId,
    errors: Vec<ScanError>,
}
```

- Full paths are reconstructed by walking parents, never stored per node.
- Aggregation is a single bottom-up pass after the walk, then incremental updates on delete/rescan.
- Hardlinks: track `(dev, ino)` in a `HashSet` **only** for files with `nlink > 1`. Duplicates get `HARDLINK_DUP` and contribute 0 to aggregates.
- Keep `Node` small. If memory exceeds the PRD target, the first optimizations are: intern names, replace `children: Vec` with a contiguous child range after sorting.

## Scanner

- Entry point: `scan(root, ScanOptions, progress: Sender<ScanEvent>, cancel: Arc<AtomicBool>) -> Result<Tree>`.
- `ScanOptions`: `cross_filesystems: bool` (default false), `apparent: bool`, `skip: Vec<PathBuf>`.
- Always skip `/proc`, `/sys`, `/dev`, `/run` and other pseudo filesystems.
- Emit `ScanEvent::Progress { files, bytes, current_path, errors }` at most every 100 ms, plus `ScanEvent::Batch` so the UI can render partial results.
- Check the cancel flag between directory reads. Cancelling returns the partial tree marked as incomplete.
- Allocated size: Unix `st_blocks * 512`; Windows `GetCompressedFileSizeW`. Put this in `size.rs` behind `cfg`.

## IPC contract (Tauri)

Commands (all return `Result<T, String>` with serializable DTOs, not core types):

| Command | Purpose |
|---------|---------|
| `list_disks()` | Disks with capacity/used/free |
| `start_scan(path, opts) -> scan_id` | Starts scan on a background thread |
| `cancel_scan(scan_id)` | Sets cancel flag |
| `get_node(id)` | Node details + full path |
| `get_children(id, sort, offset, limit)` | Paged children |
| `get_treemap(id, depth, min_bytes)` | Pruned subtree for layout; children below `min_bytes` merged into one "N smaller items" node |
| `get_largest(kind, limit)` | Largest files or folders |
| `search(query, filters, limit)` | Name/ext/size/date search |
| `get_lens(lens, params)` | File types, dev artifacts, caches, apps, dupes |
| `get_suggestions()` | Rule matches |
| `tray_add(ids)`, `tray_remove(ids)`, `tray_list()` | Cleanup tray |
| `execute_cleanup(mode)` | Runs tray through safety pipeline |
| `list_snapshots(root)`, `diff_snapshots(a, b)` | History |

Events: `scan://progress`, `scan://batch`, `scan://done`, `cleanup://progress`, `cleanup://done`.

App state: `Mutex<AppState>` holding the current `Tree`, the tray, and scan handles. Hold locks briefly; never across a scan or a cleanup run.

## Cleanup pipeline

`execute_cleanup` runs each tray item through, in order:

1. `exists_in_tree(id)`: item came from the scan.
2. `is_protected(path)`: protected list check (see PRD 6.7). Keep the list in `cleanup/protected.rs` with per-OS entries and tests for each entry.
3. `revalidate(id)`: `symlink_metadata` again; `dev` + `ino` must match; path must still be inside the scan root after canonicalizing the **parent** (not the item itself, which may be a symlink).
4. Execute: `trash::delete` or `fs::remove_dir_all` / `fs::remove_file`. For permanent dir removal, walk manually so symlinks inside are unlinked, not followed.
5. Append to the JSONL action log, update the tree, emit progress.

A failure at any step skips that item and records why. Never retry with weaker checks.

Measure freed space as the volume free-space delta before vs after (via `sysinfo`), and report it alongside the estimate.

## Suggestions / rules

Rules live in `rules/default.toml`:

```toml
[[rule]]
id = "node_modules"
name = "Node dependencies"
match = { dir_name = "node_modules", sibling = "package.json" }
safety = "safe"            # safe | review | caution
explain = "Installed npm packages for a project."
regenerate = "Run npm/pnpm/yarn install in the project."
```

Matchers supported: `dir_name`, `sibling`, `contains_file`, `path_under` (with `~` expansion), `ext_in`, `older_than_days`, `min_bytes`. Adding a rule should never require engine changes; if it does, extend the matcher set and test it. Every rule needs a fixture test that proves it matches and a test that proves it doesn't match a near-miss (e.g. a `target/` dir without `Cargo.toml`).

## Testing

- `hoard-core/tests/fixtures.rs`: a builder that creates trees in a `TempDir` with files of exact sizes, nested dirs, hardlinks, symlinks (including one pointing outside the root and a symlink loop), and on Unix a dir with `chmod 000`.
- Required test cases before M0 is done: hardlink counted once, symlink not followed, loop doesn't hang, permission-denied dir recorded not fatal, totals match expected allocated size.
- Required before M2 is done: symlink to outside root deletes only the link; protected path rejected; item swapped after scan (different inode) rejected; ancestor of scan root rejected; partial failure leaves the rest of the tray processed and reported.
- Benchmarks: `criterion` bench scanning a generated 100k-file tree. Record the baseline in `docs/benchmarks.md` and note regressions over 10%.
- Frontend: vitest for treemap layout helpers and formatting (bytes, dates).

## Frontend conventions

- TypeScript strict, no `any`.
- Treemap and sunburst render to `<canvas>`, not SVG. Layout via `d3-hierarchy` on the pruned data from `get_treemap`.
- Sizes always shown as text (`1.4 GB`) with binary units by default (GiB toggle optional); colors are never the only signal.
- Keyboard: arrows move selection, Enter drills in, Backspace goes up, Space adds to tray, `/` focuses search.
- All destructive buttons live in the tray panel only.

## Code style

- Rust 2021, stable toolchain. Small modules, explicit types on public APIs, doc comments on every public item in `hoard-core`.
- Platform code only inside `platform/` or `cfg`-gated functions with a common signature; no scattered `#[cfg]` in business logic.
- Commit messages: imperative, scoped (`core/scan: count hardlinks once`).
- In docs and UI copy, avoid em dashes.

## When unsure

- Ambiguous requirement: check the PRD first, then ask rather than guess on anything touching deletion.
- Choosing between faster and safer in cleanup code: always safer.
