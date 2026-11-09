fn main() {
    // `generate_context!` requires the frontend dist dir to exist at compile
    // time. Create it empty so `cargo check/clippy --workspace` works on a
    // fresh clone before `pnpm build` has run. `pnpm tauri dev` serves from
    // the Vite dev server instead.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist");
    if !dist.exists() {
        let _ = std::fs::create_dir_all(&dist);
    }
    tauri_build::build()
}
