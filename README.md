# Hoard

Cross-platform disk usage visualizer and cleaner, written in Rust. See
[docs/PRD.md](docs/PRD.md) for what and why, and [CLAUDE.md](CLAUDE.md) for
how the code is organised.

## Status

| Milestone | State |
|-----------|-------|
| M0 Core + CLI | Done |
| M1 Desktop viewer | Not started |
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

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo bench -p hoard-core
```
