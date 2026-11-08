//! End-to-end tests for the `hoard` binary.

use std::process::Command;

fn hoard() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hoard"))
}

#[test]
fn scan_json_reports_exact_apparent_totals() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("big")).unwrap();
    std::fs::write(dir.path().join("big").join("a.bin"), vec![1u8; 300_000]).unwrap();
    std::fs::write(dir.path().join("small.txt"), b"hello").unwrap();

    let out = hoard()
        .args(["scan", "--json", "--apparent", "--depth", "2"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["size_apparent"], 300_005);
    assert_eq!(v["files"], 2);
    assert_eq!(v["complete"], true);
    assert_eq!(v["entries"][0]["name"], "big");
    assert_eq!(v["entries"][0]["children"][0]["name"], "a.bin");
    assert_eq!(v["entries"][1]["name"], "small.txt");
}

#[test]
fn scan_top_limits_entries_and_summarises_rest() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}")), vec![0u8; 1000 * (i + 1)]).unwrap();
    }
    let out = hoard()
        .args(["scan", "--json", "--apparent", "--top", "2"])
        .arg(dir.path())
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
    assert_eq!(v["entries"][0]["name"], "f4");
    assert_eq!(v["rest"]["count"], 3);
    assert_eq!(v["rest"]["size_apparent"], 6000);
}

#[test]
fn scan_human_output_and_missing_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.bin"), vec![0u8; 2048]).unwrap();
    let out = hoard()
        .args(["scan", "--apparent"])
        .arg(dir.path())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("2.0 KB"), "{text}");
    assert!(text.contains("x.bin"), "{text}");

    let out = hoard()
        .args(["scan"])
        .arg(dir.path().join("missing"))
        .output()
        .unwrap();
    assert!(!out.status.success());
}
