use super::*;
use proptest::prelude::*;

fn places() -> Places {
    Places::new("/Users/me", "/var/folders/ab/cd/T")
}

const NOW: i64 = 1_800_000_000;

fn facts() -> Facts {
    Facts {
        path: "/Users/me/dev/a/target".into(),
        bytes: 100,
        newest: NOW - 2 * 86_400,
        foreign: false,
        unreadable: false,
        holds_repo: false,
        other_volume: false,
        in_use: false,
        git: Git::Ignored,
        running: vec![],
        installed: vec![],
    }
}

fn day() -> Age {
    diskreap_rules::ONE_DAY
}

#[test]
fn an_old_unused_match_goes() {
    assert_eq!(judge(&facts(), day(), NOW, &places()), Verdict::Delete);
}

#[test]
fn every_hold_is_named() {
    let f = Facts {
        path: "/Users/me/target".into(),
        newest: NOW - 60,
        foreign: true,
        unreadable: true,
        holds_repo: true,
        other_volume: true,
        in_use: true,
        git: Git::NotIgnored,
        running: vec!["Google Chrome".into()],
        installed: vec!["Warp".into()],
        ..facts()
    };
    let Verdict::Hold(holds) = judge(&f, day(), NOW, &places()) else { panic!("deleted") };
    let keys: Vec<_> = holds.iter().map(Hold::key).collect();
    assert_eq!(
        keys,
        [
            "floor",
            "recent",
            "in_use",
            "running",
            "installed",
            "not_yours",
            "unreadable",
            "git_tracked",
            "holds_repo",
            "other_volume"
        ]
    );
    assert_eq!(holds[1].text(), "changed or used too recently: it may go in 24h");
}

#[test]
fn a_change_in_the_future_is_recent() {
    let f = Facts { newest: NOW + 1000, ..facts() };
    assert!(matches!(judge(&f, day(), NOW, &places()), Verdict::Hold(_)));
}

#[test]
fn a_summary_counts_and_names_the_largest() {
    let judged: Vec<Judged> = (0..12u64)
        .map(|i| Judged {
            path: format!("/p/{i}"),
            bytes: i,
            complete: true,
            verdict: if i % 2 == 0 { Verdict::Delete } else { Verdict::Hold(vec![Hold::InUse]) },
        })
        .collect();
    let s = summary(&judged, false);
    assert_eq!(s["matched"], 12);
    assert_eq!(s["deletable"], 6);
    assert_eq!(s["deletable_bytes"], 30);
    assert_eq!(s["held_bytes"], 36);
    assert_eq!(s["holds"]["in_use"], 6);
    assert_eq!(s["largest"].as_array().unwrap().len(), LARGEST);
    assert_eq!(s["largest"][0]["path"], "/p/11");
    assert_eq!(summary(&judged, true)["matches"].as_array().unwrap().len(), 12);
}

proptest! {
    // A Match goes only when it has waited its Minimum age and nothing else holds it.
    #[test]
    fn goes_only_when_old_enough(age in 0i64..200_000, in_use: bool, foreign: bool) {
        let f = Facts { newest: NOW - age, in_use, foreign, ..facts() };
        let goes = judge(&f, day(), NOW, &places()) == Verdict::Delete;
        prop_assert_eq!(goes, age >= 86_400 && !in_use && !foreign);
    }
}

#[test]
fn temporary_names_group_by_what_made_them() {
    for (name, g) in [
        ("bk-catalog-10096-0", "bk-catalog"),
        ("bk-catalog-11062-12", "bk-catalog"),
        ("bk-server-test-s3-af717c02-bfc6-4c73-ab2d", "bk-server-test-s3"),
        ("go-build3562170816", "go-build"),
        ("bk-worker-test-files", "bk-worker-test-files"),
        ("com.1password.1password", "com.1password.1password"),
        ("tmp.AbC12x", "tmp"),
        ("12345", "12345"),
        ("code_sign_clone.01rsXy", "code_sign_clone"),
    ] {
        assert_eq!(group(name), g, "{name}");
    }
}
