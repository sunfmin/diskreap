//! Help is the manual: `--help` answers all of it, `<command> --help` its
//! part, `help <topic>` a topic, and it names only commands there are.

use crate::common::Env;

/// Every command as it is run (`rule list`), read from the command clap
/// parses with, so a command added is a command tested.
fn commands() -> Vec<String> {
    fn walk(prefix: &str, command: &clap::Command, all: &mut Vec<String>) {
        for sub in command.get_subcommands() {
            let name = format!("{prefix}{}", sub.get_name());
            all.push(name.clone());
            walk(&format!("{name} "), sub, all);
        }
    }
    let mut all = Vec::new();
    walk("", &diskreap::manual().clap().unwrap(), &mut all);
    all
}

/// The commands that answer one JSON object: every one that runs, all but
/// `help`, which answers Markdown, and a group such as `rule`.
fn answering() -> Vec<String> {
    let parents: Vec<String> = commands()
        .iter()
        .filter_map(|c| c.rsplit_once(' ').map(|(parent, _)| parent.to_string()))
        .collect();
    commands().into_iter().filter(|c| c != "help" && !parents.contains(c)).collect()
}

/// Whole: every command has text, every `help:` link and placeholder names
/// something, no topic twice, every command in a group. Each fault is named.
#[test]
fn the_manual_is_whole() {
    if let Err(faults) = agent_help::check(&diskreap::manual()) {
        panic!("the manual is not whole:\n{faults}");
    }
}

#[test]
fn help_answers_the_whole_manual() {
    let env = Env::new();
    let out = env.run(&["--help"]);
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, diskreap::manual().manual().unwrap());
    assert!(out.stdout.starts_with("# `diskreap`\n"), "{}", out.stdout);
    let all = env.run(&["help"]);
    assert_eq!(all.code, 0);
    assert_eq!(all.stdout, out.stdout);
}

#[test]
fn each_command_answers_its_own_section() {
    let env = Env::new();
    let manual = env.run(&["--help"]).stdout;
    for command in commands() {
        for flag in ["--help", "-h"] {
            let mut args: Vec<&str> = command.split(' ').collect();
            args.push(flag);
            let out = env.run(&args);
            assert_eq!(out.code, 0, "{command} {flag}");
            let heading = format!("# `diskreap {command}`\n");
            assert!(out.stdout.starts_with(&heading), "{command} {flag}: {}", out.stdout);
            let text = &out.stdout[heading.len()..];
            assert!(manual.contains(text), "{command}: its section is not the manual's");
        }
    }
    for command in answering() {
        let args: Vec<&str> = command.split(' ').chain(["--help"]).collect();
        assert!(env.run(&args).stdout.contains("Answers"), "{command} says no answer");
    }
}

#[test]
fn the_manual_names_only_commands_there_are() {
    let env = Env::new();
    let manual = env.run(&["--help"]).stdout;
    let mut named: Vec<String> = manual
        .split("`diskreap ")
        .skip(1)
        .map(|rest| {
            let span = rest.split('`').next().unwrap();
            span.split_whitespace()
                .take_while(|w| w.chars().all(|c| c.is_ascii_lowercase()))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|c| !c.is_empty())
        .collect();
    named.sort();
    named.dedup();
    assert!(named.len() >= answering().len(), "{named:?}");
    for command in named {
        let args: Vec<&str> = command.split(' ').chain(["--help"]).collect();
        let out = env.run(&args);
        assert_eq!(out.code, 0, "the manual names `diskreap {command}`: {}", out.stdout);
        assert!(out.stdout.starts_with(&format!("# `diskreap {command}`\n")), "{command}");
    }
}

#[test]
fn the_short_help_is_one_screen() {
    let env = Env::new();
    let out = env.run(&["-h"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.lines().count() <= 24, "{}", out.stdout);
    for command in commands().iter().filter(|c| !c.contains(' ')) {
        assert!(out.stdout.contains(&format!("  {command} ")), "{command}: {}", out.stdout);
    }
    assert!(out.stdout.contains("diskreap --help"), "{}", out.stdout);
}

#[test]
fn a_topic_answers_on_its_own() {
    let env = Env::new();
    let out = env.run(&["help", "writing-a-rule"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.starts_with("# `diskreap help writing-a-rule`\n"), "{}", out.stdout);
    assert!(out.stdout.contains("`unless_installed`"), "{}", out.stdout);
    let out = env.run(&["help", "frobnicate"]);
    assert_eq!(out.code, 1);
    let error = out.answer["error"].as_str().unwrap();
    assert!(error.contains("`writing-a-rule`"), "{error}");
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

#[test]
fn every_example_rule_is_one_rule_check_takes() {
    let env = Env::with_rules(serde_json::json!([]));
    let blocks = agent_help::code_blocks(&diskreap::manual(), "jsonl");
    let examples: Vec<&str> = blocks
        .iter()
        .flat_map(|b| b.code.lines())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    assert!(examples.len() >= 5, "the manual shows {} example Rules", examples.len());
    for rule in examples {
        let check = env.run(&["rule", "check", rule]);
        assert_eq!(check.code, 0, "{rule}: {}", check.stdout);
        assert_eq!(
            check.answer["rule"]["id"],
            serde_json::from_str::<serde_json::Value>(rule).unwrap()["id"]
        );
    }
}
