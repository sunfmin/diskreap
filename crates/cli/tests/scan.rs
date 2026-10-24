//! scan: finds, deletes nothing.

use serde_json::json;

use crate::common::{Env, file};

#[test]
fn scan_reports_large_tmpdir_groups_and_orphans_it_does_not_cover() {
    let mut env = Env::with_rules(json!([
        {"id": "bk", "glob": "$TMPDIR/bk-*", "why": "x"},
        {"id": "cargo-target", "glob": "~/**/target", "marker": "Cargo.toml", "why": "x"}
    ]));
    // Report directories of 10 MiB and up, not a gigabyte.
    env.large = Some(10 << 20);
    let big = "x".repeat(20 << 20);
    file(&env.home.join("media/videos/a.mov"), &big);
    file(&env.home.join("dev/a/Cargo.toml"), "");
    file(&env.home.join("dev/a/target/covered"), &big);
    for i in 0..3 {
        file(&env.tmpdir.join(format!("go-build{i}12345/f")), "x");
        file(&env.tmpdir.join(format!("bk-{i}/f")), "x");
    }
    file(&env.home.join("Library/Application Support/Gone App/data"), &big);
    let scan = env.run(&["scan"]);
    assert_eq!(scan.code, 0, "{}", scan.stdout);
    let large: Vec<&str> = scan.answer["large"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["path"].as_str().unwrap())
        .collect();
    assert!(large.iter().any(|p| p.ends_with("media/videos")), "{large:?}");
    assert!(!large.iter().any(|p| p.contains("target")), "{large:?}");
    let groups = scan.answer["tmpdir"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0]["group"], "go-build");
    assert_eq!(groups[0]["entries"], 3);
    let orphans = scan.answer["orphans"].as_array().unwrap();
    assert_eq!(orphans.len(), 1, "{orphans:?}");
    assert_eq!(orphans[0]["rule"]["id"], "orphan-gone-app");
    assert!(scan.events.iter().any(|e| e["event"] == "large"));
    assert!(env.home.join("media/videos/a.mov").exists());
    assert!(env.home.join("dev/a/target/covered").exists());
}
