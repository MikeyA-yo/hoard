//! Every built-in rule: a fixture it must match and a near miss it must not.

mod fixtures;

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use fixtures::{find, scan_default, Fixture};
use hoard_core::rules::{suggest, MatchContext, RuleSet};
use hoard_core::{NodeFlags, Tree};

/// Folders the placeholders point at, all inside the fixture.
fn context(fx: &Fixture) -> MatchContext {
    MatchContext {
        home: Some(fx.path("home")),
        downloads: Some(fx.path("home/Downloads")),
        local_app_data: Some(fx.path("home/AppData/Local")),
        app_data: Some(fx.path("home/AppData/Roaming")),
        temp: Some(fx.path("tmp")),
        cache: Some(fx.path("home/.cache")),
        all_os: true,
        now: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
    }
}

fn age(path: &Path, days: u64) {
    let when = SystemTime::now() - Duration::from_secs(days * 86_400);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// Files to create: (relative path, age in days).
type Layout = &'static [(&'static str, u64)];

struct Case {
    rule: &'static str,
    /// Creates a match at `expect`.
    hit: Layout,
    expect: &'static str,
    /// Creates something similar that this rule must not match.
    miss: Layout,
}

const CASES: &[Case] = &[
    Case {
        rule: "npm_cache",
        hit: &[("home/AppData/Local/npm-cache/_cacache/x", 0)],
        expect: "home/AppData/Local/npm-cache",
        miss: &[("home/AppData/Local/npm-cachex/x", 0)],
    },
    Case {
        rule: "npm_cache",
        hit: &[("home/.npm/_cacache/x", 0)],
        expect: "home/.npm/_cacache",
        miss: &[("home/.npm/_logs/x", 0)],
    },
    Case {
        rule: "pnpm_store",
        hit: &[("home/AppData/Local/pnpm/store/v3/x", 0)],
        expect: "home/AppData/Local/pnpm/store",
        miss: &[("home/AppData/Local/pnpm/other/x", 0)],
    },
    Case {
        rule: "yarn_cache",
        hit: &[("home/AppData/Local/Yarn/Cache/x", 0)],
        expect: "home/AppData/Local/Yarn/Cache",
        miss: &[("home/AppData/Local/Yarn/Data/x", 0)],
    },
    Case {
        rule: "bun_cache",
        hit: &[("home/.bun/install/cache/x", 0)],
        expect: "home/.bun/install/cache",
        miss: &[("home/.bun/bin/x", 0)],
    },
    Case {
        rule: "cargo_registry",
        hit: &[("home/.cargo/registry/src/x", 0)],
        expect: "home/.cargo/registry/src",
        miss: &[("home/.cargo/registry/index/x", 0)],
    },
    Case {
        rule: "pip_cache",
        hit: &[("home/AppData/Local/pip/Cache/x", 0)],
        expect: "home/AppData/Local/pip/Cache",
        miss: &[("home/AppData/Local/pip/config/x", 0)],
    },
    Case {
        rule: "uv_cache",
        hit: &[("home/AppData/Local/uv/cache/x", 0)],
        expect: "home/AppData/Local/uv/cache",
        miss: &[("home/AppData/Local/uv/python/x", 0)],
    },
    Case {
        rule: "gradle_cache",
        hit: &[("home/.gradle/caches/x", 0)],
        expect: "home/.gradle/caches",
        miss: &[("home/.gradle/wrapper/x", 0)],
    },
    Case {
        rule: "go_build_cache",
        hit: &[("home/AppData/Local/go-build/x", 0)],
        expect: "home/AppData/Local/go-build",
        miss: &[("home/AppData/Local/go-builder/x", 0)],
    },
    Case {
        rule: "go_mod_cache",
        hit: &[("home/go/pkg/mod/x", 0)],
        expect: "home/go/pkg/mod",
        miss: &[("home/go/pkg/sumdb/x", 0)],
    },
    Case {
        rule: "nuget_packages",
        hit: &[("home/.nuget/packages/x", 0)],
        expect: "home/.nuget/packages",
        miss: &[("home/.nuget/config/x", 0)],
    },
    Case {
        rule: "docker_disk",
        hit: &[("home/AppData/Local/Docker/wsl/disk/docker_data.vhdx", 0)],
        expect: "home/AppData/Local/Docker/wsl/disk/docker_data.vhdx",
        miss: &[("home/vms/data.vhdx", 0)],
    },
    Case {
        rule: "node_modules",
        hit: &[("proj/node_modules/x", 0), ("proj/package.json", 0)],
        expect: "proj/node_modules",
        miss: &[("proj2/node_modules/x", 0)],
    },
    Case {
        rule: "rust_target",
        hit: &[("proj/target/x", 0), ("proj/Cargo.toml", 0)],
        expect: "proj/target",
        miss: &[("proj2/target/x", 0)],
    },
    Case {
        rule: "python_venv",
        hit: &[("proj/.venv/pyvenv.cfg", 0), ("proj/.venv/lib/x", 0)],
        expect: "proj/.venv",
        miss: &[("proj/.venv2/pyvenv.txt", 0)],
    },
    Case {
        rule: "python_caches",
        hit: &[("proj/__pycache__/a.pyc", 0)],
        expect: "proj/__pycache__",
        miss: &[("proj/pycache/a.pyc", 0)],
    },
    Case {
        rule: "js_framework_caches",
        hit: &[("web/.next/x", 0), ("web/package.json", 0)],
        expect: "web/.next",
        miss: &[("web2/.next/x", 0)],
    },
    Case {
        rule: "xcode_derived_data",
        hit: &[("home/Library/Developer/Xcode/DerivedData/x", 0)],
        expect: "home/Library/Developer/Xcode/DerivedData",
        miss: &[("home/Library/Developer/Xcode/Archives/x", 0)],
    },
    Case {
        rule: "ios_simulators",
        hit: &[("home/Library/Developer/CoreSimulator/x", 0)],
        expect: "home/Library/Developer/CoreSimulator",
        miss: &[("home/Library/Developer/Other/x", 0)],
    },
    Case {
        rule: "browser_cache",
        hit: &[(
            "home/AppData/Local/Google/Chrome/User Data/Default/Cache/x",
            0,
        )],
        expect: "home/AppData/Local/Google/Chrome/User Data/Default/Cache",
        miss: &[("home/Cache/x", 0)],
    },
    Case {
        rule: "vscode_cache",
        hit: &[("home/AppData/Roaming/Code/CachedData/x", 0)],
        expect: "home/AppData/Roaming/Code/CachedData",
        miss: &[("home/AppData/Roaming/Code/User/x", 0)],
    },
    Case {
        rule: "crash_dumps",
        hit: &[("home/AppData/Local/CrashDumps/app.dmp", 0)],
        expect: "home/AppData/Local/CrashDumps",
        miss: &[("home/AppData/Local/CrashDump/app.dmp", 0)],
    },
    Case {
        rule: "old_temp",
        hit: &[("tmp/old.tmp", 30)],
        expect: "tmp/old.tmp",
        miss: &[("tmp/fresh.tmp", 0)],
    },
    Case {
        rule: "user_logs",
        hit: &[("home/Library/Logs/x.log", 0)],
        expect: "home/Library/Logs",
        miss: &[("home/Library/Log/x.log", 0)],
    },
    Case {
        rule: "old_installers",
        hit: &[("home/Downloads/setup.exe", 0)],
        expect: "home/Downloads/setup.exe",
        miss: &[("home/Downloads/notes.txt", 0), ("home/setup.exe", 0)],
    },
    Case {
        rule: "old_downloads",
        hit: &[("home/Downloads/old.pdf", 200)],
        expect: "home/Downloads/old.pdf",
        miss: &[("home/Downloads/new.pdf", 0)],
    },
];

fn build(layout: Layout) -> (Fixture, Tree) {
    let fx = Fixture::new();
    for (rel, days) in layout {
        let p = fx.file(rel, 1_000);
        if *days > 0 {
            age(&p, *days);
        }
    }
    let tree = scan_default(fx.root());
    (fx, tree)
}

fn matched_paths(fx: &Fixture, tree: &Tree, rule: &str) -> Vec<String> {
    let rules = RuleSet::builtin().unwrap();
    suggest(tree, &rules, &context(fx))
        .into_iter()
        .filter(|m| m.rule.id == rule)
        .flat_map(|m| m.items)
        .map(|s| {
            let p = tree.path(s.node).unwrap();
            p.strip_prefix(tree.root_path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

#[test]
fn every_rule_has_a_case() {
    let rules = RuleSet::builtin().unwrap();
    for r in rules.rules() {
        assert!(
            CASES.iter().any(|c| c.rule == r.id),
            "rule {} has no fixture test",
            r.id
        );
    }
}

#[test]
fn every_rule_matches_its_fixture() {
    for case in CASES {
        let (fx, tree) = build(case.hit);
        let got = matched_paths(&fx, &tree, case.rule);
        assert_eq!(
            got,
            vec![case.expect.to_string()],
            "rule {} should match {}",
            case.rule,
            case.expect
        );
    }
}

#[test]
fn no_rule_matches_its_near_miss() {
    for case in CASES {
        let (fx, tree) = build(case.miss);
        let got = matched_paths(&fx, &tree, case.rule);
        assert!(
            got.is_empty(),
            "rule {} matched near miss: {got:?}",
            case.rule
        );
    }
}

#[test]
fn nothing_inside_a_match_is_matched_again() {
    // A node_modules inside a node_modules, and a cache inside a project.
    let (fx, tree) = build(&[
        ("proj/package.json", 0),
        ("proj/node_modules/dep/package.json", 0),
        ("proj/node_modules/dep/node_modules/x", 0),
        ("proj/node_modules/dep/__pycache__/y.pyc", 0),
    ]);
    assert_eq!(
        matched_paths(&fx, &tree, "node_modules"),
        vec!["proj/node_modules"]
    );
    assert!(matched_paths(&fx, &tree, "python_caches").is_empty());
}

#[test]
fn first_rule_wins() {
    // An installer older than 90 days is an installer, not an "old download".
    let (fx, tree) = build(&[("home/Downloads/setup.msi", 400)]);
    assert_eq!(
        matched_paths(&fx, &tree, "old_installers"),
        vec!["home/Downloads/setup.msi"]
    );
    assert!(matched_paths(&fx, &tree, "old_downloads").is_empty());
}

#[test]
fn removed_nodes_are_not_suggested() {
    let (fx, mut tree) = build(&[("proj/package.json", 0), ("proj/node_modules/x", 0)]);
    let nm = find(&tree, "proj/node_modules").unwrap();
    tree.remove_subtree(nm);
    assert!(tree.get(nm).unwrap().flags.contains(NodeFlags::REMOVED));
    assert!(matched_paths(&fx, &tree, "node_modules").is_empty());
}

#[test]
fn path_rules_do_not_fire_when_their_folder_is_missing() {
    // With no AppData at all, path rules must not match arbitrary folders.
    let (fx, tree) = build(&[("somewhere/Cache/x", 0), ("somewhere/store/x", 0)]);
    let rules = RuleSet::builtin().unwrap();
    let all = suggest(&tree, &rules, &context(&fx));
    assert!(
        all.is_empty(),
        "unexpected: {:?}",
        all.iter().map(|m| &m.rule.id).collect::<Vec<_>>()
    );
}

#[test]
fn groups_are_sorted_by_size_and_totals_add_up() {
    let fx = Fixture::new();
    fx.file("a/package.json", 10);
    fx.file("a/node_modules/x", 300_000);
    fx.file("b/package.json", 10);
    fx.file("b/node_modules/x", 100_000);
    fx.file("c/__pycache__/y.pyc", 50_000);
    let tree = scan_default(fx.root());
    let rules = RuleSet::builtin().unwrap();
    let groups = suggest(&tree, &rules, &context(&fx));
    assert_eq!(groups[0].rule.id, "node_modules");
    assert_eq!(groups[0].items.len(), 2);
    assert!(groups[0].items[0].size_apparent > groups[0].items[1].size_apparent);
    assert_eq!(groups[0].total_apparent, 400_000);
    assert_eq!(groups[1].rule.id, "python_caches");
}
