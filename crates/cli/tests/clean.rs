//! plan and clean: what goes, and above all what stays.

use serde_json::{Value, json};

use crate::common::{Env, cargo_target_rule, file, git};

fn deleted_paths(out: &crate::common::Out) -> Vec<String> {
    out.events
        .iter()
        .filter(|e| e["event"] == "deleted")
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect()
}

fn holds_of(out: &crate::common::Out, path: &std::path::Path) -> Vec<String> {
    let p = path.to_string_lossy();
    out.events
        .iter()
        .find(|e| e["event"] == "match" && e["path"] == *p)
        .unwrap_or_else(|| panic!("no event for {p}: {:?}", out.events))["holds"]
        .as_array()
        .map(|h| h.iter().map(|s| s.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

#[test]
fn an_old_cargo_target_goes_and_its_project_stays() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "[package]");
    file(&proj.join("src/main.rs"), "fn main() {}");
    file(&proj.join("target/debug/a"), "binary");
    env.later();

    let plan = env.run(&["plan"]);
    assert_eq!(plan.code, 0, "{}", plan.stdout);
    let r = &plan.answer["rules"]["cargo-target"];
    assert_eq!(r["matched"], 1);
    assert_eq!(r["deletable"], 1);
    assert!(proj.join("target").exists(), "plan deletes nothing");
    assert_eq!(plan.events.first().unwrap()["event"], "start");
    assert_eq!(plan.events.iter().filter(|e| e["event"] == "match").count(), 1);

    let clean = env.clean();
    assert_eq!(clean.code, 0, "{}", clean.stdout);
    assert_eq!(clean.answer["deleted"], 1);
    assert_eq!(deleted_paths(&clean), [proj.join("target").to_string_lossy()]);
    assert!(!proj.join("target").exists());
    assert!(proj.join("src/main.rs").exists());
    assert!(proj.join("Cargo.toml").exists());
    assert!(clean.answer["free_before"].is_u64() && clean.answer["freed_bytes"].is_i64());

    // Run again: nothing left to do.
    let again = env.clean();
    assert_eq!(again.answer["matched"], 0);
}

#[test]
fn a_target_with_no_marker_is_not_a_match() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let precious = env.home.join("photos/target/me.jpg");
    file(&precious, "me");
    env.later();
    let clean = env.clean();
    assert_eq!(clean.answer["matched"], 0);
    assert!(precious.exists());
}

#[test]
fn a_recent_change_anywhere_inside_holds_it() {
    let env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    file(&proj.join("target/deep/deeper/new.o"), "just built");
    let clean = env.clean();
    assert_eq!(clean.answer["rules"]["cargo-target"]["holds"]["recent"], 1);
    assert!(holds_of(&clean, &proj.join("target"))[0].starts_with("changed or used too recently"));
    assert!(proj.join("target/deep/deeper/new.o").exists());
}

#[test]
fn a_file_read_lately_holds_it_though_nothing_changed() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    let used = proj.join("target/debug/a");
    file(&used, "read every day");
    env.later();
    let now = env.now.unwrap();
    // Read an hour before the run: its access time moves, its writes do not.
    let read = std::time::UNIX_EPOCH + std::time::Duration::from_secs((now - 3600) as u64);
    let f = std::fs::File::options().write(true).open(&used).unwrap();
    f.set_times(std::fs::FileTimes::new().set_accessed(read)).unwrap();
    drop(f);
    let clean = env.clean();
    assert_eq!(clean.answer["rules"]["cargo-target"]["holds"]["recent"], 1, "{}", clean.stdout);
    assert!(holds_of(&clean, &proj.join("target"))[0].starts_with("changed or used too recently"));
    assert!(used.exists());
}

#[test]
fn an_open_file_holds_it() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    file(&proj.join("target/debug/server"), "running");
    env.set("open", json!([proj.join("target/debug/server")]));
    env.later();
    let clean = env.clean();
    assert_eq!(clean.answer["rules"]["cargo-target"]["holds"]["in_use"], 1);
    assert!(proj.join("target/debug/server").exists());
}

#[test]
fn a_process_working_in_it_holds_it_and_a_similar_name_does_not() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let a = env.home.join("dev/a");
    let b = env.home.join("dev/b");
    for p in [&a, &b] {
        file(&p.join("Cargo.toml"), "");
        file(&p.join("target/x"), "");
    }
    // A process works in a/target; another holds `b/target2`, which is not in b/target.
    env.set("open", json!([a.join("target"), b.join("target2/y")]));
    env.later();
    env.clean();
    assert!(a.join("target").exists());
    assert!(!b.join("target").exists());
}

#[test]
fn git_tracked_output_stays_and_ignored_output_goes() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let tracked = env.home.join("dev/tracked");
    let ignored = env.home.join("dev/ignored");
    for repo in [&tracked, &ignored] {
        file(&repo.join("Cargo.toml"), "");
        file(&repo.join("target/out"), "");
        git(repo, &["init", "-q"]);
    }
    git(&tracked, &["add", "."]);
    git(&tracked, &["commit", "-qm", "x"]);
    file(&ignored.join(".gitignore"), "target/\n");
    git(&ignored, &["add", "."]);
    git(&ignored, &["commit", "-qm", "x"]);
    env.later();
    let clean = env.clean();
    assert_eq!(clean.answer["rules"]["cargo-target"]["holds"]["git_tracked"], 1);
    assert!(tracked.join("target/out").exists());
    assert!(!ignored.join("target").exists());
}

#[test]
fn a_file_forced_into_git_inside_an_ignored_dir_holds_it() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let repo = env.home.join("dev/r");
    file(&repo.join("Cargo.toml"), "");
    file(&repo.join(".gitignore"), "target/\n");
    file(&repo.join("target/keep.txt"), "forced");
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["add", "-f", "target/keep.txt"]);
    git(&repo, &["commit", "-qm", "x"]);
    env.later();
    env.clean();
    assert!(repo.join("target/keep.txt").exists());
}

#[test]
fn a_repository_inside_holds_it() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    file(&proj.join("target/vendored/work.rs"), "unpushed");
    git(&proj.join("target/vendored"), &["init", "-q"]);
    env.later();
    let clean = env.clean();
    assert_eq!(clean.answer["rules"]["cargo-target"]["holds"]["holds_repo"], 1);
    assert!(proj.join("target/vendored/work.rs").exists());
}

#[test]
fn a_symlink_match_goes_alone_and_symlinks_inside_are_not_followed() {
    let mut env = Env::with_rules(json!([
        cargo_target_rule(),
        {"id": "bk", "glob": "$TMPDIR/bk-*", "why": "test leftovers"}
    ]));
    let precious = env.home.join("Documents/thesis");
    file(&precious.join("chapter1.md"), "years of work");
    // A target that is a link to the thesis.
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    std::os::unix::fs::symlink(&precious, proj.join("target")).unwrap();
    // A temporary directory with a link inside it to the thesis.
    let tmp = env.tmpdir.join("bk-1");
    file(&tmp.join("x"), "");
    std::os::unix::fs::symlink(&precious, tmp.join("link")).unwrap();
    env.later();
    let clean = env.clean();
    assert_eq!(clean.code, 0, "{}", clean.stdout);
    assert!(!tmp.exists(), "{:?}", clean.events);
    assert!(std::fs::symlink_metadata(proj.join("target")).is_err());
    assert_eq!(std::fs::read_to_string(precious.join("chapter1.md")).unwrap(), "years of work");
}

#[test]
fn a_glob_does_not_reach_through_a_symlinked_directory() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let elsewhere = env.home.join("elsewhere/proj");
    file(&elsewhere.join("Cargo.toml"), "");
    file(&elsewhere.join("target/x"), "");
    std::fs::create_dir_all(env.home.join("dev")).unwrap();
    std::os::unix::fs::symlink(env.home.join("elsewhere"), env.home.join("dev/link")).unwrap();
    env.later();
    let plan = env.run(&["plan", "--all"]);
    let paths: Vec<&str> = plan.answer["rules"]["cargo-target"]["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, [elsewhere.join("target").to_string_lossy()]);
}

#[test]
fn the_floor_keeps_shallow_matches() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    // ~/target, beside ~/Cargo.toml: directly in the home directory.
    file(&env.home.join("Cargo.toml"), "");
    file(&env.home.join("target/x"), "");
    env.later();
    let clean = env.clean();
    assert_eq!(clean.answer["rules"]["cargo-target"]["holds"]["floor"], 1);
    assert!(env.home.join("target/x").exists());
}

#[test]
fn any_depth_does_not_go_into_library_or_trash() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    for base in ["Library/x", ".Trash/y"] {
        let proj = env.home.join(base);
        file(&proj.join("Cargo.toml"), "");
        file(&proj.join("target/z"), "");
    }
    env.later();
    let clean = env.clean();
    assert_eq!(clean.answer["matched"], 0);
    assert!(env.home.join("Library/x/target/z").exists());
}

#[test]
fn unless_running_holds_while_the_program_runs() {
    let mut env = Env::with_rules(json!([{
        "id": "chrome-clone",
        "glob": "$VARFOLDER/X/com.google.Chrome.code_sign_clone/*",
        "unless_running": ["Google Chrome"],
        "why": "Chrome makes a new one each launch",
    }]));
    let clone = env.var_folder.join("X/com.google.Chrome.code_sign_clone/code_sign_clone.01rsXy");
    file(&clone.join("Google Chrome.app/Contents/Info.plist"), "");
    env.set("running", json!(["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]));
    env.later();
    let clean = env.clean();
    assert_eq!(clean.answer["rules"]["chrome-clone"]["holds"]["running"], 1);
    assert!(clone.exists());
    env.set("running", json!(["/usr/bin/login"]));
    let clean = env.clean();
    assert_eq!(clean.answer["deleted"], 1);
    assert!(!clone.exists());
    assert!(env.var_folder.join("X/com.google.Chrome.code_sign_clone").exists());
}

#[test]
fn unless_installed_holds_while_the_app_is_there() {
    let mut env = Env::with_rules(json!([{
        "id": "orphan-warp",
        "glob": "~/Library/Application Support/dev.warp.Warp-Stable",
        "unless_installed": ["Warp", "dev.warp.Warp-Stable"],
        "why": "Warp is uninstalled",
    }]));
    let data = env.home.join("Library/Application Support/dev.warp.Warp-Stable");
    file(&data.join("db"), "");
    std::fs::create_dir_all(env.home.join("Applications/Warp.app")).unwrap();
    env.later();
    let clean = env.clean();
    assert_eq!(clean.answer["rules"]["orphan-warp"]["holds"]["installed"], 1);
    assert!(data.exists());
    // Gone from ~/Applications, but Spotlight still finds its bundle id.
    std::fs::remove_dir_all(env.home.join("Applications/Warp.app")).unwrap();
    env.set("bundles", json!(["dev.warp.Warp-Stable"]));
    env.clean();
    assert!(data.exists());
    env.set("bundles", json!([]));
    env.clean();
    assert!(!data.exists());
}

#[test]
fn clean_deletes_as_it_finds_and_names_each_in_turn() {
    let mut env =
        Env::with_rules(json!([{"id": "bk", "glob": "$TMPDIR/bk-*", "why": "test leftovers"}]));
    for i in 0..5 {
        file(&env.tmpdir.join(format!("bk-{i}/f")), "x");
    }
    file(&env.tmpdir.join("other/f"), "x");
    env.later();
    let clean = env.clean();
    let kinds: Vec<&str> = clean.events.iter().map(|e| e["event"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["start", "deleted", "deleted", "deleted", "deleted", "deleted", "rule"]);
    assert!(env.tmpdir.join("other/f").exists());
    assert_eq!(clean.answer["rules"]["bk"]["deleted"], 5);
}

#[test]
fn plan_names_the_largest_ten_or_all() {
    let mut env =
        Env::with_rules(json!([{"id": "bk", "glob": "$TMPDIR/bk-*", "why": "test leftovers"}]));
    for i in 0..12 {
        file(&env.tmpdir.join(format!("bk-{i}")), &"x".repeat(5000 * (i + 1)));
    }
    env.later();
    let plan = env.run(&["plan"]);
    let largest = plan.answer["rules"]["bk"]["largest"].as_array().unwrap();
    assert_eq!(largest.len(), 10);
    assert!(largest[0]["path"].as_str().unwrap().ends_with("bk-11"));
    let all = env.run(&["plan", "--all", "--rule", "bk"]);
    assert_eq!(all.answer["rules"]["bk"]["matches"].as_array().unwrap().len(), 12);
    let none = env.run(&["plan", "--rule", "nope"]);
    assert_eq!(none.code, 1);
    assert!(none.answer["error"].as_str().unwrap().contains("the ids are bk"));
}

#[test]
fn lsof_failing_stops_the_run_before_anything_goes() {
    let mut env =
        Env::with_rules(json!([{"id": "bk", "glob": "$TMPDIR/bk-*", "why": "test leftovers"}]));
    file(&env.tmpdir.join("bk-1/f"), "x");
    env.set("fail", json!({"lsof": "no"}));
    env.later();
    let clean = env.clean();
    assert_eq!(clean.code, 1);
    assert!(clean.answer["error"].as_str().unwrap().contains("lsof"));
    assert!(env.tmpdir.join("bk-1/f").exists());
}

#[test]
fn every_byte_count_is_a_number() {
    let mut env =
        Env::with_rules(json!([{"id": "bk", "glob": "$TMPDIR/bk-*", "why": "test leftovers"}]));
    file(&env.tmpdir.join("bk-1/f"), "x");
    env.later();
    let plan = env.run(&["plan"]);
    for key in ["matched", "deletable", "deletable_bytes"] {
        assert!(plan.answer[key].is_u64(), "{key}: {}", plan.stdout);
    }
    assert!(plan.answer["note"].is_string());
    let _: &Value = &plan.answer["seconds"];
}

#[test]
fn rules_searching_from_the_same_place_share_a_walk_and_each_finds_its_own() {
    let mut env = Env::with_rules(json!([
        cargo_target_rule(),
        {"id": "node-modules", "glob": "~/**/node_modules", "marker": "package.json", "why": "npm installs it again"},
        {"id": "bk", "glob": "$TMPDIR/bk-*", "why": "test leftovers"}
    ]));
    let rust = env.home.join("dev/rust");
    let web = env.home.join("dev/web");
    file(&rust.join("Cargo.toml"), "");
    file(&rust.join("target/x"), "");
    file(&web.join("package.json"), "");
    file(&web.join("node_modules/left-pad/index.js"), "");
    // node_modules inside a target is not looked for: the target is a Match.
    file(&rust.join("target/package.json"), "");
    file(&rust.join("target/node_modules/y"), "");
    file(&env.tmpdir.join("bk-1/f"), "");
    env.later();
    let plan = env.run(&["plan", "--all"]);
    let paths = |id: &str| -> Vec<String> {
        plan.answer["rules"][id]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(paths("cargo-target"), [rust.join("target").to_string_lossy()]);
    assert_eq!(paths("node-modules"), [web.join("node_modules").to_string_lossy()]);
    assert_eq!(paths("bk").len(), 1);
    let starts = plan.events.iter().filter(|e| e["event"] == "start").count();
    let ends = plan.events.iter().filter(|e| e["event"] == "rule").count();
    assert_eq!((starts, ends), (3, 3));
}

#[test]
fn clean_with_a_path_deletes_that_match_alone() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let [a, b] = ["a", "b"].map(|n| env.home.join("dev").join(n));
    for p in [&a, &b] {
        file(&p.join("Cargo.toml"), "");
        file(&p.join("target/debug/x"), "built");
    }
    env.later();
    let target = a.join("target");
    let clean = env.run(&["clean", "--path", target.to_str().unwrap()]);
    assert_eq!(clean.code, 0, "{}", clean.stdout);
    assert_eq!(clean.answer["matched"], 1);
    assert_eq!(clean.answer["deleted"], 1);
    assert_eq!(clean.answer["paths"], serde_json::json!([target]));
    assert_eq!(clean.answer["unmatched"], serde_json::json!([]));
    assert!(!target.exists());
    assert!(b.join("target/debug/x").exists(), "another Match stays");
}

#[test]
fn a_path_no_rule_matches_is_kept_and_says_why() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    file(&proj.join("src/main.rs"), "fn main() {}");
    file(&proj.join("target/debug/x"), "built");
    let unmarked = env.home.join("photos/target");
    file(&unmarked.join("me.jpg"), "me");
    let hidden = env.home.join("Library/x/target");
    file(&env.home.join("Library/x/Cargo.toml"), "");
    file(&hidden.join("debug/x"), "built");
    env.later();
    for path in [
        proj.join("src"),
        proj.clone(),
        proj.join("target/debug"),
        unmarked.clone(),
        hidden.clone(),
    ] {
        let clean = env.run(&["clean", "--path", path.to_str().unwrap()]);
        assert_eq!(clean.code, 0, "{}", clean.stdout);
        assert_eq!(clean.answer["matched"], 0, "{}", path.display());
        assert!(clean.answer["why"].as_str().unwrap().starts_with("no Rule matches"));
        assert!(path.exists(), "{}", path.display());
    }
}

#[test]
fn a_path_through_a_symlink_or_dots_is_refused() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    file(&proj.join("target/debug/x"), "built");
    std::os::unix::fs::symlink(&proj, env.home.join("link")).unwrap();
    env.later();
    for path in
        [env.home.join("link/target"), proj.join("src/../target"), env.home.join("dev/a/gone")]
    {
        let clean = env.run(&["clean", "--path", path.to_str().unwrap()]);
        assert_ne!(clean.code, 0, "{}", clean.stdout);
        assert!(clean.answer["error"].is_string());
    }
    assert!(proj.join("target/debug/x").exists());
}

#[test]
fn a_path_is_held_as_a_walk_would_hold_it() {
    let env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    file(&proj.join("target/debug/x"), "just built");
    let target = proj.join("target");
    let clean = env.run(&["clean", "--path", target.to_str().unwrap()]);
    assert_eq!(clean.answer["matched"], 1);
    assert_eq!(clean.answer["deleted"], 0);
    assert_eq!(clean.answer["rules"]["cargo-target"]["holds"]["recent"], 1);
    assert!(target.exists());
}

#[test]
fn what_tools_installed_in_hidden_directories_is_not_a_match() {
    let rule = json!({"id": "node-modules", "glob": "~/**/node_modules",
        "marker": ["package-lock.json", "pnpm-lock.yaml"], "why": "installed again"});
    let mut env = Env::with_rules(json!([rule]));
    let tool = env.home.join(".pi/agent/install/releases/1.2.0");
    file(&tool.join("package.json"), "{}");
    file(&tool.join("package-lock.json"), "{}");
    file(&tool.join("node_modules/x/index.js"), "");
    let unlocked = env.home.join("dev/published-package");
    file(&unlocked.join("package.json"), "{}");
    file(&unlocked.join("node_modules/x/index.js"), "");
    let project = env.home.join("dev/app");
    file(&project.join("package.json"), "{}");
    file(&project.join("pnpm-lock.yaml"), "");
    file(&project.join("node_modules/x/index.js"), "");
    env.later();
    let clean = env.clean();
    assert_eq!(clean.code, 0, "{}", clean.stdout);
    assert_eq!(deleted_paths(&clean), [project.join("node_modules").to_string_lossy()]);
    assert!(tool.join("node_modules/x/index.js").exists());
    assert!(unlocked.join("node_modules/x/index.js").exists());
}

#[test]
fn clean_with_no_paths_deletes_nothing_and_says_how() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let proj = env.home.join("dev/a");
    file(&proj.join("Cargo.toml"), "");
    file(&proj.join("target/debug/x"), "built");
    env.later();
    let clean = env.run(&["clean"]);
    assert_ne!(clean.code, 0, "{}", clean.stdout);
    assert!(clean.answer["error"].as_str().unwrap().contains("diskreap plan --all"));
    assert!(proj.join("target/debug/x").exists());
}

#[test]
fn clean_reads_the_reviewed_paths_from_stdin() {
    let mut env = Env::with_rules(json!([cargo_target_rule()]));
    let [a, b, c] = ["a", "b", "c"].map(|n| env.home.join("dev").join(n));
    for p in [&a, &b, &c] {
        file(&p.join("Cargo.toml"), "");
        file(&p.join("target/debug/x"), "built");
    }
    env.later();
    // The review kept b: only a and c are handed to clean.
    let list = format!("{}\n\n{}\n", a.join("target").display(), c.join("target").display());
    let clean = env.run_in(&["clean", "--path", "-"], Some(&list));
    assert_eq!(clean.code, 0, "{}", clean.stdout);
    assert_eq!(clean.answer["deleted"], 2);
    assert!(!a.join("target").exists() && !c.join("target").exists());
    assert!(b.join("target/debug/x").exists());
}
