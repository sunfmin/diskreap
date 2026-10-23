use super::*;
use proptest::prelude::*;
use serde_json::json;

fn places() -> Places {
    Places::new("/Users/me", "/var/folders/ab/cd/T/")
}

fn rule(value: Value) -> Result<Rule, Vec<String>> {
    parse_rule(&value, &places())
}

#[test]
fn places_find_the_var_folder_above_t() {
    let p = places();
    assert_eq!(p.tmpdir, "/var/folders/ab/cd/T");
    assert_eq!(p.var_folder, "/var/folders/ab/cd");
    assert_eq!(Places::new("/h", "/tmp/x").var_folder, "/tmp/x");
}

#[test]
fn ages_read_and_write_back() {
    for (text, secs) in [("30m", 1800), ("12h", 43_200), ("1d", 86_400), ("2w", 1_209_600)] {
        let age = parse_age(text).unwrap();
        assert_eq!(age.secs(), secs);
        assert_eq!(age_text(age), text);
    }
    assert_eq!(age_text(parse_age("14d").unwrap()), "2w");
    for bad in ["", "d", "1", "1y", "-1d", "0d", "1.5d", "999999w", "99999999999999999999d"] {
        assert!(parse_age(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_rule_defaults_to_a_day_and_writes_back_as_read() {
    let r = rule(json!({"id": "bk-tmp", "glob": "$TMPDIR/bk-*", "why": "test leftovers"})).unwrap();
    assert_eq!(r.min_age, ONE_DAY);
    assert_eq!(
        rule_json(&r),
        json!({"id": "bk-tmp", "glob": "$TMPDIR/bk-*", "min_age": "1d", "why": "test leftovers"})
    );
    let list = list_text(std::slice::from_ref(&r));
    assert_eq!(parse_list(&list, &places()).unwrap(), vec![r]);
}

#[test]
fn any_depth_needs_a_marker() {
    let faults = rule(json!({"id": "t", "glob": "~/**/target", "why": "cargo"})).unwrap_err();
    assert!(faults[0].contains("names no marker"), "{faults:?}");
    rule(json!({"id": "t", "glob": "~/**/target", "marker": "Cargo.toml", "why": "cargo"}))
        .unwrap();
}

#[test]
fn every_fault_is_named_at_once() {
    let faults =
        rule(json!({"id": "Bad Id", "glob": "/etc/*", "min_age": "1y", "extra": 1})).unwrap_err();
    assert_eq!(faults.len(), 5, "{faults:?}");
}

#[test]
fn globs_that_reach_the_floor_are_refused() {
    for glob in [
        "/tmp/x",
        "~/*",
        "~/Developments",
        "~/Library/*",
        "~/Library/Caches",
        "$TMPDIR",
        "$VARFOLDER/*",
        "$VARFOLDER/T",
        "~/a/../b",
        "~/a//b",
        "~/a/**",
        "~/a**b/c",
    ] {
        assert!(compile(glob, &places()).is_err(), "{glob}");
    }
    for glob in [
        "~/Developments/**/target",
        "~/**/target",
        "~/Library/Caches/go-build",
        "~/Library/Application Support/Type4Me",
        "$TMPDIR/bk-*",
        "$VARFOLDER/X/com.google.Chrome.code_sign_clone/*",
    ] {
        assert!(compile(glob, &places()).is_ok(), "{glob}");
    }
}

#[test]
fn globs_match_paths() {
    let g = compile("~/**/target", &places()).unwrap();
    assert!(path_matches(&g, "/Users/me/dev/a/target"));
    assert!(path_matches(&g, "/Users/me/target"));
    assert!(!path_matches(&g, "/Users/me/dev/a/target/x"));
    assert!(!path_matches(&g, "/Users/meow/target"));
    let g = compile("$TMPDIR/bk-*-test", &places()).unwrap();
    assert!(path_matches(&g, "/var/folders/ab/cd/T/bk-1-test"));
    assert!(path_matches(&g, "/var/folders/ab/cd/T/bk--test"));
    assert!(!path_matches(&g, "/var/folders/ab/cd/T/bk-1-tests"));
    let g = compile("~/Library/Caches/go-build", &places()).unwrap();
    assert!(path_matches(&g, "/Users/me/Library/Caches/go-build"));
}

#[test]
fn the_floor() {
    let p = places();
    for kept in [
        "/",
        "/etc/hosts",
        "/Users/me",
        "/Users/me/Developments",
        "/Users/me/Library",
        "/Users/me/Library/Caches",
        "/Users/meow/a/b",
        "/var/folders/ab/cd",
        "/var/folders/ab/cd/T",
        "/var/folders/ab/other/T/x",
        "/Users/me/a/../b",
    ] {
        assert!(floor(kept, &p).is_some(), "{kept}");
    }
    for judged in ["/Users/me/a/b", "/Users/me/Library/Caches/x", "/var/folders/ab/cd/T/bk-1"] {
        assert_eq!(floor(judged, &p), None, "{judged}");
    }
}

#[test]
fn any_depth_skips_library_trash_and_git() {
    let p = places();
    let home = compile("~/**/node_modules", &p).unwrap();
    assert!(skipped("/Users/me", "Library", &home, &p));
    assert!(skipped("/Users/me", ".Trash", &home, &p));
    assert!(skipped("/Users/me/x", ".git", &home, &p));
    assert!(!skipped("/Users/me", "Developments", &home, &p));
    assert!(!skipped("/Users/me/x", "Library", &home, &p));
    let lib = compile("~/Library/Developer/**/DerivedData", &p).unwrap();
    assert!(!skipped("/Users/me", "Library", &lib, &p));
}

#[test]
fn a_walk_reaches_a_match_only_through_directories_it_goes_into() {
    let p = places();
    let home = compile("~/**/node_modules", &p).unwrap();
    assert!(reached(&home, "/Users/me/dev/a/node_modules", &p));
    assert!(!reached(&home, "/Users/me/dev/a/node_modules/x", &p));
    assert!(!reached(&home, "/Users/me/Library/x/node_modules", &p));
    assert!(!reached(&home, "/Users/me/.Trash/node_modules", &p));
    assert!(!reached(&home, "/Users/me/dev/.git/x/node_modules", &p));
    assert!(!reached(&home, "/Users/other/node_modules", &p));
    let lib = compile("~/Library/Developer/**/DerivedData", &p).unwrap();
    assert!(reached(&lib, "/Users/me/Library/Developer/Xcode/DerivedData", &p));
    let tmp = compile("$TMPDIR/go-build*", &p).unwrap();
    assert!(reached(&tmp, "/var/folders/ab/cd/T/go-build123", &p));
    assert!(!reached(&tmp, "/var/folders/ab/cd/T/x/go-build123", &p));
}

proptest! {
    // A name with no wildcard matches only itself, and `*` matches every name.
    #[test]
    fn literal_and_star(name in "[a-z.-]{0,12}", other in "[a-z.-]{0,12}") {
        let n: Vec<char> = name.chars().collect();
        let o: Vec<char> = other.chars().collect();
        prop_assert_eq!(name_matches(&pieces(&name), &o), name == other);
        prop_assert!(name_matches(&[Piece::Many], &n));
    }

    // Whatever the path, the floor keeps it unless it is two names deep in
    // the home directory (three in ~/Library) or in the var folder.
    #[test]
    fn floor_keeps_shallow_paths(names in proptest::collection::vec("[a-zA-Z]{1,5}", 0..4)) {
        let p = places();
        let path = if names.is_empty() { p.home.clone() } else { format!("{}/{}", p.home, names.join("/")) };
        let deep = names.len() >= 2 && !(names[0] == "Library" && names.len() < 3);
        prop_assert_eq!(floor(&path, &p).is_none(), deep);
    }
}
