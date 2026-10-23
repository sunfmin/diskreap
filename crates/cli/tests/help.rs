//! Help is the manual: it names exactly the commands there are.

use crate::common::Env;

const COMMANDS: [&str; 10] = [
    "init",
    "doctor",
    "plan",
    "clean",
    "scan",
    "rule list",
    "rule check",
    "rule add",
    "rule set",
    "rule rm",
];

#[test]
fn the_guide_names_exactly_the_commands() {
    let env = Env::new();
    let guide = env.run(&["--help"]).stdout;
    let mut named: Vec<String> = guide
        .lines()
        .filter_map(|l| l.trim().strip_prefix("diskreap "))
        .map(|rest| {
            let words: Vec<&str> = rest
                .split_whitespace()
                .take_while(|w| w.chars().all(|c| c.is_ascii_lowercase()))
                .collect();
            words
                .iter()
                .take(if words.first() == Some(&"rule") { 2 } else { 1 })
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|c| !c.is_empty() && c != "rule")
        .collect();
    named.sort();
    named.dedup();
    let mut want: Vec<String> = COMMANDS.iter().map(|c| c.to_string()).collect();
    want.sort();
    assert_eq!(named, want);
}

#[test]
fn each_command_has_its_own_help() {
    let env = Env::new();
    for command in COMMANDS {
        let mut args: Vec<&str> = command.split(' ').collect();
        args.push("--help");
        let out = env.run(&args);
        assert_eq!(out.code, 0);
        assert!(
            out.stdout.starts_with(&format!("diskreap {command}")),
            "{command}: {}",
            out.stdout
        );
        assert!(out.stdout.contains("ANSWERS"), "{command}");
    }
    assert!(env.run(&["rule", "--help"]).stdout.starts_with("diskreap rule list|"));
    let screen = env.run(&["-h"]).stdout;
    assert!(screen.lines().count() <= 24);
}

#[test]
fn version_and_unknown_commands() {
    let env = Env::new();
    let v = env.run(&["--version"]).stdout;
    assert!(v.starts_with("diskreap 0."), "{v}");
    let out = env.run(&["frobnicate"]);
    assert_eq!(out.code, 1);
    assert!(out.answer["error"].as_str().unwrap().contains("diskreap --help"));
}
