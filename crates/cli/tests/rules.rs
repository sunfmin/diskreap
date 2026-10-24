//! init and the Rule list in the gist.

use serde_json::json;

use crate::common::{Env, cargo_target_rule};

#[test]
fn init_makes_one_private_gist_and_then_finds_it() {
    let env = Env::new();
    let first = env.run(&["init"]);
    assert_eq!(first.code, 0, "{}", first.stdout);
    assert_eq!(first.answer["created"], true);
    assert_eq!(first.answer["rules"], 0);
    let again = env.run(&["init"]);
    assert_eq!(again.answer["created"], false);
    assert_eq!(again.answer["gist"], first.answer["gist"]);
    assert_eq!(env.state()["gists"].as_object().unwrap().len(), 1);
}

#[test]
fn with_no_gist_commands_say_to_init() {
    let env = Env::new();
    let out = env.run(&["plan"]);
    assert_eq!(out.code, 1);
    assert!(out.answer["error"].as_str().unwrap().contains("diskreap init"), "{}", out.stdout);
}

#[test]
fn add_check_set_rm() {
    let env = Env::with_rules(json!([]));
    let rule = cargo_target_rule().to_string();
    let check = env.run(&["rule", "check", &rule]);
    assert_eq!(check.code, 0, "{}", check.stdout);
    assert_eq!(check.answer["id_taken"], false);
    assert_eq!(check.answer["plan"]["matched"], 0);
    assert_eq!(env.gist_rules(), json!([]), "check writes nothing");

    let add = env.run(&["rule", "add", &rule]);
    assert_eq!(add.answer["added"], true, "{}", add.stdout);
    let written = json!([{"id": "cargo-target", "glob": "~/**/target", "marker": "Cargo.toml", "min_age": "1d", "why": "cargo builds it again"}]);
    assert_eq!(env.gist_rules(), written);

    let same = env.run_in(&["rule", "add", "-"], Some(&rule));
    assert_eq!(same.answer["added"], false, "{}", same.stdout);
    let other =
        json!({"id": "cargo-target", "glob": "~/**/out", "marker": "Cargo.toml", "why": "x"});
    let clash = env.run(&["rule", "add", &other.to_string()]);
    assert_eq!(clash.code, 1);
    assert!(clash.answer["error"].as_str().unwrap().contains("a different Rule has the id"));

    let set = env.run(&["rule", "set", "cargo-target", "--min-age", "2w"]);
    assert_eq!(set.answer["changed"], true, "{}", set.stdout);
    assert_eq!(env.gist_rules()[0]["min_age"], "2w");
    let unchanged = env.run(&["rule", "set", "cargo-target", "--min-age", "14d"]);
    assert_eq!(unchanged.answer["changed"], false);

    let list = env.run(&["rule", "list"]);
    assert_eq!(list.answer["rules"][0]["id"], "cargo-target");

    let rm = env.run(&["rule", "rm", "cargo-target"]);
    assert_eq!(rm.answer["removed"], true);
    assert_eq!(env.gist_rules(), json!([]));
    let rm = env.run(&["rule", "rm", "cargo-target"]);
    assert_eq!(rm.answer["removed"], false);
    assert_eq!(rm.code, 0);
}

#[test]
fn a_wrong_rule_is_refused_naming_every_fault() {
    let env = Env::with_rules(json!([]));
    let bad = json!({"id": "X", "glob": "~/**/node_modules", "min_age": "soon", "color": 1});
    let out = env.run(&["rule", "add", &bad.to_string()]);
    assert_eq!(out.code, 1);
    let error = out.answer["error"].as_str().unwrap();
    for fault in
        ["no key `color`", "the id `X`", "needs `why`", "the age `soon`", "names no marker"]
    {
        assert!(error.contains(fault), "{fault}: {error}");
    }
    for glob in ["/etc/*", "~/*", "~/Library/*", "$TMPDIR", "~/a/../b"] {
        let r = json!({"id": "x", "glob": glob, "why": "x"});
        let out = env.run(&["rule", "check", &r.to_string()]);
        assert_eq!(out.code, 1, "{glob}: {}", out.stdout);
    }
    assert_eq!(env.gist_rules(), json!([]));
}

#[test]
fn a_gist_changed_meanwhile_is_not_overwritten() {
    let env = Env::with_rules(json!([]));
    env.set("bump_on_read", json!(true));
    let out = env.run(&["rule", "add", &cargo_target_rule().to_string()]);
    assert_eq!(out.code, 1);
    assert!(out.answer["error"].as_str().unwrap().contains("changed while this ran"));
    assert_eq!(env.gist_rules(), json!([]));
}

#[test]
fn a_wrong_rule_list_in_the_gist_is_named() {
    let env = Env::with_rules(
        json!([{"id": "a", "glob": "/", "why": "x"}, {"id": "a", "glob": "$TMPDIR/x", "why": "x"}]),
    );
    let out = env.run(&["plan"]);
    assert_eq!(out.code, 1);
    let error = out.answer["error"].as_str().unwrap();
    assert!(error.contains("is wrong") && error.contains("does not start at"), "{error}");
}

#[test]
fn gh_failing_says_how_to_fix_it() {
    let env = Env::new();
    env.set("fail", json!({"gh": "HTTP 401: Bad credentials"}));
    let out = env.run(&["init"]);
    assert_eq!(out.code, 1);
    assert!(out.answer["error"].as_str().unwrap().contains("gh auth login"));
}

#[test]
fn doctor_names_each_finding() {
    let env = Env::new();
    let out = env.run(&["doctor"]);
    assert_eq!(out.code, 0);
    assert_eq!(out.answer["ok"], false);
    let gist = out.answer["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["check"] == "gist")
        .unwrap()
        .clone();
    assert!(gist["fix"].as_str().unwrap().contains("diskreap init"));
    env.run(&["init"]);
    let out = env.run(&["doctor"]);
    assert_eq!(out.answer["ok"], true, "{}", out.stdout);
    assert_eq!(out.answer["places"]["$TMPDIR"], env.tmpdir.to_string_lossy().as_ref());
}

fn listings(env: &Env) -> usize {
    env.state()["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| {
            c.as_array()
                .unwrap()
                .iter()
                .any(|a| a.as_str().is_some_and(|a| a.starts_with("/gists?")))
        })
        .count()
}

#[test]
fn the_gist_found_once_is_remembered_and_looked_for_again_when_gone() {
    let env = Env::new();
    let init = env.run(&["init"]);
    let cache = env.home.join(".config/diskreap/gist");
    let id = init.answer["gist"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(&cache).unwrap().trim(), id);
    let before = listings(&env);
    env.run(&["rule", "list"]);
    env.run(&["plan"]);
    assert_eq!(listings(&env), before, "a remembered gist is read, not looked for");

    // The cache names a gist that is gone: it is looked for, and remembered anew.
    std::fs::write(&cache, "deadbeef\n").unwrap();
    let list = env.run(&["rule", "list"]);
    assert_eq!(list.code, 0, "{}", list.stdout);
    assert_eq!(list.answer["gist"], id);
    assert_eq!(std::fs::read_to_string(&cache).unwrap().trim(), id);
    assert!(listings(&env) > before);

    // No cache at all is the same as a first run.
    std::fs::remove_file(&cache).unwrap();
    assert_eq!(env.run(&["init"]).answer["created"], false);
    assert!(cache.exists());
}
