//! App attribution against a fixture tree (no registry access).

mod fixtures;

use std::path::PathBuf;

use fixtures::{find, scan_default, Fixture};
use hoard_core::apps::{attribute, Confidence, InstalledApp};
use hoard_core::rules::MatchContext;

fn app(name: &str, publisher: Option<&str>, install: Option<PathBuf>) -> InstalledApp {
    InstalledApp {
        name: name.into(),
        publisher: publisher.map(Into::into),
        version: None,
        install_location: install,
        estimated_size: None,
    }
}

#[test]
fn attributes_install_and_data_folders() {
    let fx = Fixture::new();
    fx.file("Programs/Discord/app.exe", 50_000);
    fx.file("home/AppData/Roaming/discord/Cache/c.bin", 30_000);
    fx.file("home/AppData/Local/Discord/packages/p.nupkg", 20_000);
    fx.file("home/AppData/Local/Google/Chrome/User Data/x", 40_000);
    fx.file("home/AppData/Local/Google/DriveFS/y", 10_000);
    fx.file("home/AppData/Roaming/Code/User/settings.json", 5_000);
    fx.file("home/AppData/Local/Microsoft/Teams/t", 7_000);
    fx.file("home/AppData/Local/Unrelated/u", 9_000);
    let tree = scan_default(fx.root());
    let ctx = MatchContext {
        home: Some(fx.path("home")),
        app_data: Some(fx.path("home/AppData/Roaming")),
        local_app_data: Some(fx.path("home/AppData/Local")),
        ..MatchContext::default()
    };
    let apps = vec![
        app(
            "Discord",
            Some("Discord Inc."),
            Some(fx.path("Programs/Discord")),
        ),
        app("Google Chrome", Some("Google LLC"), None),
        app("Google Drive", Some("Google LLC"), None),
        app(
            "Microsoft Visual Studio Code",
            Some("Microsoft Corporation"),
            None,
        ),
        app("Microsoft Teams", Some("Microsoft Corporation"), None),
        app(
            "Not Installed Here",
            None,
            Some(PathBuf::from("Z:\\nowhere")),
        ),
    ];
    let out = attribute(&tree, &apps, &ctx);
    let by_name = |n: &str| out.iter().find(|f| f.app.name == n);

    let discord = by_name("Discord").expect("discord");
    let install = discord.install.as_ref().unwrap();
    assert_eq!(install.node, find(&tree, "Programs/Discord").unwrap());
    assert_eq!(install.confidence, Confidence::Exact);
    assert_eq!(discord.data.len(), 2, "Roaming/discord and Local/Discord");
    assert!(discord
        .data
        .iter()
        .all(|d| d.confidence == Confidence::Likely));

    // Publisher folder "Google" holds two apps: each claims only its subfolder.
    let chrome = by_name("Google Chrome").expect("chrome");
    assert_eq!(
        chrome.data[0].node,
        find(&tree, "home/AppData/Local/Google/Chrome").unwrap()
    );
    // The shared publisher folder itself is never claimed. (Google Drive keeps
    // its data in "DriveFS", which name matching cannot know: no attribution.)
    let google = find(&tree, "home/AppData/Local/Google").unwrap();
    assert!(out.iter().all(|f| f.data.iter().all(|d| d.node != google)));

    // "Code" folder goes to VS Code; the generic "Microsoft" folder is never claimed.
    let code = by_name("Microsoft Visual Studio Code").expect("vscode");
    assert_eq!(
        code.data[0].node,
        find(&tree, "home/AppData/Roaming/Code").unwrap()
    );
    let teams = by_name("Microsoft Teams");
    assert!(teams.is_none_or(|t| t
        .data
        .iter()
        .all(|d| d.node != find(&tree, "home/AppData/Local/Microsoft").unwrap())));

    // Nothing claims an unrelated folder, and apps with no footprint are dropped.
    let unrelated = find(&tree, "home/AppData/Local/Unrelated").unwrap();
    assert!(out
        .iter()
        .all(|f| f.data.iter().all(|d| d.node != unrelated)));
    assert!(by_name("Not Installed Here").is_none());
    // Each folder belongs to at most one app.
    let mut all: Vec<_> = out
        .iter()
        .flat_map(|f| f.data.iter().map(|d| d.node))
        .collect();
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n);
}
