//! Docker disk usage (PRD dev artifacts table, M5).
//!
//! Asks the Docker engine for its usage (`docker system df`) and looks for
//! Docker Desktop's disk image on disk. Read-only: Hoard never removes Docker
//! data itself. Each category comes with the Docker command that frees it,
//! for the user to run, and on Windows the steps to give the freed space back
//! to the drive (the disk image does not shrink on its own).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::platform;
use crate::rules::Safety;

/// How long to wait for `docker system df`. It answers in about 2 s when
/// the engine runs and fails fast when it does not, but took over 20 s on a
/// busy machine and can hang while Docker Desktop is starting.
const DF_TIMEOUT: Duration = Duration::from_secs(60);

/// Everything Hoard knows about Docker's disk usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerReport {
    /// What the engine reported, or why it could not be asked.
    pub engine: Engine,
    /// Docker Desktop disk images found on this machine.
    pub disk_images: Vec<DiskImage>,
    /// One command that frees images, containers and build cache together
    /// (never volumes).
    pub prune_all: &'static str,
}

/// The Docker engine's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Engine {
    /// The engine answered; its usage per category.
    Running {
        /// Images, containers, volumes and build cache, in that order.
        categories: Vec<DockerCategory>,
    },
    /// No `docker` command was found.
    NotInstalled,
    /// `docker` exists but the engine refused (usually: Docker Desktop is
    /// not running).
    Unavailable {
        /// Docker's own message.
        reason: String,
    },
    /// The engine was asked but did not answer in time (starting up, or the
    /// machine is busy).
    TimedOut {
        /// How long Hoard waited.
        seconds: u64,
    },
}

/// What kind of Docker data a category holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CategoryKind {
    /// Images (downloaded or built).
    Images,
    /// Containers (their writable layers).
    Containers,
    /// Volumes (persistent data).
    Volumes,
    /// `docker build` layer cache.
    BuildCache,
}

/// One row of `docker system df`, with what to do about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockerCategory {
    /// Which category.
    pub kind: CategoryKind,
    /// Display name.
    pub name: &'static str,
    /// Number of items.
    pub total: u64,
    /// Items in use (by a container, or for containers: running).
    pub active: u64,
    /// Total size in bytes. Docker reports 4 significant digits, so this is
    /// accurate to about 0.1%.
    pub size: u64,
    /// Bytes not in use, which the command can free.
    pub reclaimable: u64,
    /// How careful to be.
    pub safety: Safety,
    /// The command that frees (or for volumes, lists) the unused items.
    pub command: &'static str,
    /// What the command removes and what is lost.
    pub explain: &'static str,
}

/// A Docker Desktop disk image (the virtual disk all Docker data lives in).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskImage {
    /// Where it is.
    pub path: PathBuf,
    /// Space it takes on the drive.
    pub allocated: u64,
    /// Its logical size.
    pub apparent: u64,
    /// Steps to shrink it after pruning, when it does not shrink on its own.
    pub compact: Option<Vec<Step>>,
}

/// One step of a manual procedure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// What to do.
    pub text: String,
    /// What to type, if anything (may span several lines).
    pub command: Option<String>,
}

/// Collects the full report: runs `docker system df` (blocking, up to
/// about 20 s) and stats the known disk image locations.
pub fn report() -> DockerReport {
    let engine = match run_df() {
        DfOutput::NotFound => Engine::NotInstalled,
        DfOutput::Failed(reason) => Engine::Unavailable { reason },
        DfOutput::TimedOut(seconds) => Engine::TimedOut { seconds },
        DfOutput::Ok(stdout) => match parse_df(&stdout) {
            Some(categories) => Engine::Running { categories },
            None => Engine::Unavailable {
                reason: "Docker's answer could not be read".into(),
            },
        },
    };
    let disk_images = platform::docker_disk_images()
        .into_iter()
        .filter_map(|p| disk_image(&p))
        .collect();
    DockerReport {
        engine,
        disk_images,
        prune_all: PRUNE_ALL,
    }
}

const PRUNE_ALL: &str = "docker system prune -a";

/// What one category's row says, before Hoard adds its advice.
struct Row {
    kind: CategoryKind,
    total: u64,
    active: u64,
    size: u64,
    reclaimable: u64,
}

/// Parses `docker system df --format json`: one JSON object per line, with
/// every value a string, e.g.
/// `{"Active":"2","Reclaimable":"1.2GB (45%)","Size":"2.7GB","TotalCount":"5","Type":"Images"}`.
/// Unknown types are ignored; `None` if no known row could be read.
fn parse_df(stdout: &str) -> Option<Vec<DockerCategory>> {
    let mut rows: Vec<Row> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| parse_row(l.trim()))
        .collect();
    if rows.is_empty() {
        return None;
    }
    let order = |k: CategoryKind| ORDER.iter().position(|&o| o == k);
    rows.sort_by_key(|r| order(r.kind));
    rows.dedup_by_key(|r| r.kind);
    Some(rows.into_iter().map(advise).collect())
}

const ORDER: [CategoryKind; 4] = [
    CategoryKind::Images,
    CategoryKind::Containers,
    CategoryKind::Volumes,
    CategoryKind::BuildCache,
];

fn parse_row(line: &str) -> Option<Row> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let field = |k: &str| match v.get(k)? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    let kind = match field("Type")?.as_str() {
        "Images" => CategoryKind::Images,
        "Containers" => CategoryKind::Containers,
        "Local Volumes" | "Volumes" => CategoryKind::Volumes,
        "Build Cache" => CategoryKind::BuildCache,
        _ => return None,
    };
    let count = |k: &str| field(k).and_then(|s| s.trim().parse::<u64>().ok());
    let size = parse_size(&field("Size")?)?;
    // "1.2GB (45%)", or just "0B" when the category is empty.
    let reclaimable = field("Reclaimable")
        .and_then(|s| s.split_whitespace().next().and_then(parse_size))
        .unwrap_or(0)
        .min(size);
    Some(Row {
        kind,
        total: count("TotalCount").unwrap_or(0),
        active: count("Active").unwrap_or(0),
        size,
        reclaimable,
    })
}

/// Parses Docker's human sizes: decimal units as printed by the engine
/// (`"0B"`, `"12.29kB"`, `"2.701GB"`), plus binary units (`"1.5GiB"`) for
/// safety. Case-insensitive.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let num: f64 = num.parse().ok()?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "pb" => 1e15,
        "kib" => 1024.0,
        "mib" => 1024f64.powi(2),
        "gib" => 1024f64.powi(3),
        "tib" => 1024f64.powi(4),
        _ => return None,
    };
    let bytes = (num * mult).round();
    (bytes.is_finite() && bytes >= 0.0).then_some(bytes as u64)
}

/// Adds Hoard's advice to a parsed row.
fn advise(r: Row) -> DockerCategory {
    let (name, safety, command, explain) = match r.kind {
        CategoryKind::Images => (
            "Images",
            Safety::Review,
            "docker image prune -a",
            "Removes images no container uses. Docker downloads or rebuilds an image again when something needs it.",
        ),
        CategoryKind::Containers => (
            "Containers",
            Safety::Review,
            "docker container prune",
            "Removes stopped containers, including any changes made inside them. Their volumes are kept.",
        ),
        CategoryKind::Volumes => (
            "Volumes",
            Safety::Caution,
            "docker volume ls --filter dangling=true",
            "Volumes hold data such as databases and are never recreated. This lists the ones no container uses; remove one only when you know what it holds, with docker volume rm NAME.",
        ),
        CategoryKind::BuildCache => (
            "Build cache",
            Safety::Safe,
            "docker builder prune -a",
            "Removes layers saved to speed up docker build. The next build recreates them, more slowly the first time.",
        ),
    };
    DockerCategory {
        kind: r.kind,
        name,
        total: r.total,
        active: r.active,
        size: r.size,
        reclaimable: r.reclaimable,
        safety,
        command,
        explain,
    }
}

/// Describes the disk image at `path`, if there is one.
fn disk_image(path: &Path) -> Option<DiskImage> {
    let meta = platform::stat(path).ok()?;
    if meta.kind != crate::tree::NodeKind::File {
        return None;
    }
    let shrinks_by_itself = platform::is_sparse(path);
    Some(DiskImage {
        path: path.to_path_buf(),
        allocated: meta.size_allocated,
        apparent: meta.size_apparent,
        compact: (is_vhdx(path) && !shrinks_by_itself).then(|| compact_steps(path)),
    })
}

fn is_vhdx(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vhdx"))
}

/// How to shrink a WSL disk image after pruning (Windows). Compacting gives
/// back blocks that are free inside the image; it must not be in use.
fn compact_steps(path: &Path) -> Vec<Step> {
    let step = |text: &str, command: Option<String>| Step {
        text: text.to_string(),
        command,
    };
    vec![
        step(
            "Free space inside Docker first with the prune commands for each category (Docker Desktop must be running). Compacting only gives back space that is already free inside the disk image.",
            None,
        ),
        step(
            "Quit Docker Desktop: right-click its icon in the taskbar and choose Quit Docker Desktop.",
            None,
        ),
        step(
            "Stop WSL so nothing is using the disk image.",
            Some("wsl --shutdown".into()),
        ),
        step(
            "Open PowerShell as administrator, run diskpart, and type these lines one by one.",
            Some(format!(
                "select vdisk file=\"{}\"\nattach vdisk readonly\ncompact vdisk\ndetach vdisk\nexit",
                path.display()
            )),
        ),
        step("Start Docker Desktop again.", None),
    ]
}

/// The outcome of running `docker system df`.
enum DfOutput {
    TimedOut(u64),
    NotFound,
    Failed(String),
    Ok(String),
}

fn run_df() -> DfOutput {
    let mut last_err = None;
    for exe in docker_executables() {
        let mut cmd = Command::new(&exe);
        cmd.args(["system", "df", "--format", "json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::hide_console(&mut cmd);
        match run_with_timeout(cmd, DF_TIMEOUT) {
            Ok(out) => return out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => last_err = Some(e),
        }
    }
    match last_err {
        Some(e) => DfOutput::Failed(format!("docker could not be started: {e}")),
        None => DfOutput::NotFound,
    }
}

/// `docker` from PATH first, then where Docker usually installs it (apps
/// started from a desktop shell may not see the user's PATH).
fn docker_executables() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("docker")];
    v.extend(
        platform::docker_cli_candidates()
            .into_iter()
            .filter(|p| p.is_file()),
    );
    v
}

/// Runs `cmd`, killing it after `timeout`. Output is read on helper threads
/// so a chatty child never blocks on a full pipe.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<DfOutput> {
    let mut child = cmd.spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    Ok(match status {
        None => DfOutput::TimedOut(timeout.as_secs()),
        Some(s) if s.success() => DfOutput::Ok(stdout),
        Some(_) => DfOutput::Failed(first_line(&stderr)),
    })
}

/// The first meaningful line of Docker's error output.
fn first_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("the Docker engine did not answer")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DF: &str = r#"{"Active":"2","Reclaimable":"1.234GB (45%)","Size":"2.701GB","TotalCount":"5","Type":"Images"}
{"Active":"0","Reclaimable":"12.29kB (100%)","Size":"12.29kB","TotalCount":"3","Type":"Containers"}
{"Active":"1","Reclaimable":"0B (0%)","Size":"512MB","TotalCount":"2","Type":"Local Volumes"}
{"Active":"0","Reclaimable":"3.5GB","Size":"3.5GB","TotalCount":"41","Type":"Build Cache"}
"#;

    #[test]
    fn sizes_parse_like_docker_prints_them() {
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("12.29kB"), Some(12_290));
        assert_eq!(parse_size("2.701GB"), Some(2_701_000_000));
        assert_eq!(parse_size("1.5TB"), Some(1_500_000_000_000));
        assert_eq!(parse_size("512MB"), Some(512_000_000));
        assert_eq!(parse_size("1GiB"), Some(1 << 30));
        assert_eq!(parse_size(" 7 b "), Some(7));
        assert_eq!(parse_size("lots"), None);
        assert_eq!(parse_size("1.2XB"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn parses_system_df_json() {
        let cats = parse_df(DF).unwrap();
        let kinds: Vec<_> = cats.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, ORDER);
        let images = &cats[0];
        assert_eq!((images.total, images.active), (5, 2));
        assert_eq!(images.size, 2_701_000_000);
        assert_eq!(images.reclaimable, 1_234_000_000);
        assert_eq!(cats[1].reclaimable, 12_290);
        assert_eq!(cats[2].reclaimable, 0);
        // Build cache without a percentage.
        assert_eq!(cats[3].reclaimable, 3_500_000_000);
    }

    #[test]
    fn rows_in_any_order_and_unknown_types_are_tolerated() {
        let df = r#"{"Type":"Build Cache","TotalCount":"1","Active":"0","Size":"1MB","Reclaimable":"1MB"}
not json
{"Type":"Networks","TotalCount":"9","Active":"1","Size":"0B","Reclaimable":"0B"}
{"Type":"Images","TotalCount":3,"Active":1,"Size":"2GB","Reclaimable":"9GB (450%)"}"#;
        let cats = parse_df(df).unwrap();
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].kind, CategoryKind::Images);
        assert_eq!(cats[0].total, 3);
        // Reclaimable never exceeds the size.
        assert_eq!(cats[0].reclaimable, 2_000_000_000);
        assert!(parse_df("").is_none());
        assert!(parse_df("Cannot connect to the Docker daemon").is_none());
    }

    #[test]
    fn advice_never_removes_volumes() {
        let cats = parse_df(DF).unwrap();
        for c in &cats {
            assert!(c.command.starts_with("docker "), "{}", c.command);
            assert!(!c.command.contains("--volumes"), "{}", c.command);
        }
        let volumes = cats
            .iter()
            .find(|c| c.kind == CategoryKind::Volumes)
            .unwrap();
        assert_eq!(volumes.safety, Safety::Caution);
        // For volumes Hoard only suggests listing them.
        assert!(volumes.command.starts_with("docker volume ls"));
        assert!(!PRUNE_ALL.contains("volume"));
        let cache = cats
            .iter()
            .find(|c| c.kind == CategoryKind::BuildCache)
            .unwrap();
        assert_eq!(cache.safety, Safety::Safe);
    }

    #[test]
    fn compaction_steps_name_the_image() {
        let p = Path::new(r"C:\Users\me\AppData\Local\Docker\wsl\disk\docker_data.vhdx");
        let steps = compact_steps(p);
        let commands: Vec<&str> = steps.iter().filter_map(|s| s.command.as_deref()).collect();
        assert_eq!(commands[0], "wsl --shutdown");
        assert!(commands[1].starts_with(&format!("select vdisk file=\"{}\"", p.display())));
        assert!(commands[1].contains("attach vdisk readonly\ncompact vdisk\ndetach vdisk"));
        assert!(is_vhdx(Path::new("x/EXT4.VHDX")));
        assert!(!is_vhdx(Path::new("x/Docker.raw")));
    }

    #[test]
    fn disk_image_is_described_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("docker_data.vhdx");
        std::fs::write(&img, vec![1u8; 10_000]).unwrap();
        let d = disk_image(&img).unwrap();
        assert_eq!(d.apparent, 10_000);
        assert!(d.allocated >= 10_000);
        assert!(d.compact.is_some());
        assert_eq!(std::fs::read(&img).unwrap().len(), 10_000);
        // A folder or a missing file is not a disk image.
        assert!(disk_image(dir.path()).is_none());
        assert!(disk_image(&dir.path().join("missing.vhdx")).is_none());
        // A raw image (macOS, sparse) gets no compaction steps.
        let raw = dir.path().join("Docker.raw");
        std::fs::write(&raw, b"x").unwrap();
        assert!(disk_image(&raw).unwrap().compact.is_none());
    }

    /// A command that runs for about 10 seconds.
    fn slow_command() -> Command {
        let mut c = if cfg!(windows) {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 10"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("10");
            c
        };
        c.stdout(Stdio::piped()).stderr(Stdio::piped());
        c
    }

    #[test]
    fn a_hung_command_is_cut_off() {
        let t = Instant::now();
        let out = run_with_timeout(slow_command(), Duration::from_millis(300)).unwrap();
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        match out {
            DfOutput::TimedOut(secs) => assert_eq!(secs, 0),
            _ => panic!("expected a timeout"),
        }
    }

    #[test]
    fn a_missing_command_is_not_found() {
        let cmd = Command::new("hoard-test-no-such-command-xyz");
        let err = run_with_timeout(cmd, Duration::from_secs(1)).err().unwrap();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn engine_errors_report_the_first_line() {
        assert_eq!(
            first_line("\n  error during connect: no pipe\nmore\n"),
            "error during connect: no pipe"
        );
        assert_eq!(first_line(""), "the Docker engine did not answer");
    }

    #[test]
    fn report_serializes_with_a_status_tag() {
        let r = DockerReport {
            engine: Engine::Unavailable {
                reason: "not running".into(),
            },
            disk_images: vec![],
            prune_all: PRUNE_ALL,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["engine"]["status"], "unavailable");
        assert_eq!(v["pruneAll"], "docker system prune -a");
    }
}
