//! The effect layer: reads the world, asks the pure crates what to make of
//! it, acts, and prints the answer. Every command prints one JSON object on
//! stdout and, while it walks, one JSON event a line on stderr; `--help`,
//! `-h`, `help` and `--version` print text.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_help::Help;
use anyhow::{Result, anyhow, bail};
use clap::FromArgMatches;
use diskreap_rules::{FILE, Root, Rule, parse_age, parse_list, parse_rule, rule_json};
use serde_json::{Value, json};

mod cli;
mod gh;
mod par;
mod run;
mod scan;
mod system;
mod walk;
mod world;

/// What this binary was built from, as `--version` says it: stamped by `build.rs`.
pub const VERSION: &str = env!("DISKREAP_VERSION");

pub use cli::manual;
use cli::{Cli, Command, RuleCommand};

agent_help::binary!("diskreap");

/// Runs diskreap with these arguments, the program's name first.
pub fn main(args: Vec<OsString>) -> ExitCode {
    if let Some(screen) = cli::screen(&args) {
        print!("{screen}");
        return ExitCode::SUCCESS;
    }
    let help = manual();
    let tool = match help.clap() {
        Ok(command) => command,
        Err(e) => return fail(&format!("{e}")),
    };
    let cli = match tool.try_get_matches_from(&args).and_then(|m| Cli::from_arg_matches(&m)) {
        Ok(cli) => cli,
        // `--help`, `-h` after a command, and `--version`: their text, on stdout.
        Err(e) if !e.use_stderr() => {
            print!("{}", e.render());
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let said = e.to_string();
            let first = said.lines().next().unwrap_or("").trim_start_matches("error: ");
            return fail(&format!(
                "{first}: run `diskreap --help` for the commands and what they take"
            ));
        }
    };
    match command(cli.command, &help) {
        Ok(said) => {
            print!("{said}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("{e:#}")),
    }
}

fn fail(message: &str) -> ExitCode {
    println!("{}", json!({ "error": message }));
    ExitCode::FAILURE
}

/// What the command says on stdout: the manual's Markdown for `help`, else
/// its one JSON object on a line.
fn command(command: Command, help: &Help) -> Result<String> {
    let answer = match command {
        Command::Help { topic: None } => return Ok(help.manual()?),
        Command::Help { topic: Some(topic) } => return Ok(help.topic_help(&topic)?),
        Command::Init => init()?,
        Command::Doctor => doctor(&world::read()?),
        Command::Plan { rule, path, all } => {
            plan(&world::read()?, rule.as_deref(), path, false, all)?
        }
        Command::Clean { rule, path, all } => {
            plan(&world::read()?, rule.as_deref(), path, true, all)?
        }
        Command::Scan => {
            let world = world::read()?;
            let (gist, rules) = gh::load(&world.places)?;
            scan::scan(&world, &gist.id, &rules)?
        }
        Command::Rule(command) => rule(&world::read()?, command)?,
    };
    Ok(format!("{answer}\n"))
}

fn init() -> Result<Value> {
    let cache = gh::cache().map(|p| p.to_string_lossy().into_owned());
    match gh::open() {
        Ok(gist) => {
            let count = serde_json::from_str::<Value>(&gist.text)
                .ok()
                .and_then(|v| v["rules"].as_array().map(Vec::len));
            Ok(
                json!({"gist": gist.id, "created": false, "rules": count, "file": FILE, "cache": cache}),
            )
        }
        Err(_) if gh::find()?.is_empty() => {
            let id = gh::create()?;
            Ok(json!({"gist": id, "created": true, "rules": 0, "file": FILE, "cache": cache}))
        }
        Err(e) => Err(e),
    }
}

fn plan(
    world: &world::World,
    only: Option<&str>,
    path: Vec<PathBuf>,
    delete: bool,
    all: bool,
) -> Result<Value> {
    let path = paths(path)?;
    if delete && path.is_empty() {
        bail!(
            "clean deletes only the paths it is given: run `diskreap plan --all`, look over every path it would delete and drop any that is not a build or a cache, then `diskreap clean --path <path>`... (or the paths a line each on stdin with `--path -`)"
        );
    }
    let (gist, rules) = gh::load(&world.places)?;
    let chosen: Vec<Rule> =
        match only {
            None => rules,
            Some(id) => vec![rules.iter().find(|r| r.id == id).cloned().ok_or_else(|| {
                anyhow!("no Rule has the id `{id}`: the ids are {}", ids(&rules))
            })?],
        };
    run::rules(world, &gist.id, &chosen, path, delete, all)
}

/// The paths named, `-` read as a path a line from stdin, each made real;
/// every one that is not is named at once.
fn paths(given: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut named = Vec::new();
    for p in given {
        if p == Path::new("-") {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            named.extend(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from));
        } else {
            named.push(p);
        }
    }
    let (mut real_paths, mut faults) = (Vec::new(), Vec::new());
    for p in named {
        match real(&p) {
            Ok(r) => real_paths.push(r),
            Err(e) => faults.push(e.to_string()),
        }
    }
    if !faults.is_empty() {
        bail!("{}", faults.join("; "));
    }
    real_paths.sort();
    real_paths.dedup();
    Ok(real_paths)
}

/// The path as it is on disk, refused when it is not there or is reached
/// through a symlink or `.` or `..`: what is deleted is what was named. The
/// path itself may be a symlink, a Match of its own: deleting it removes
/// the link alone.
fn real(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        bail!("{} names no entry in a directory: give the path of one", path.display());
    };
    let real_parent = std::fs::canonicalize(parent).map_err(|e| {
        anyhow!("{} cannot be read ({e}): give a path that is there", path.display())
    })?;
    if real_parent != parent {
        bail!(
            "{} goes through a symlink or is written with `.` or `..`: give its real path, {}",
            path.display(),
            real_parent.join(name).display()
        );
    }
    let real = real_parent.join(name);
    std::fs::symlink_metadata(&real).map_err(|e| {
        anyhow!("{} cannot be read ({e}): give a path that is there", path.display())
    })?;
    Ok(real)
}

fn ids(rules: &[Rule]) -> String {
    if rules.is_empty() {
        "none yet (`diskreap rule add`)".into()
    } else {
        rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join(", ")
    }
}

/// A Rule given on the command line, or on stdin as `-`.
fn given(text: &str, world: &world::World) -> Result<Rule> {
    let text = if text == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        text.to_string()
    };
    let value: Value = serde_json::from_str(&text).map_err(|e| {
        anyhow!("the Rule is not JSON ({e}): give it like '{{\"id\": \"cargo-target\", \"glob\": \"~/**/target\", \"marker\": \"Cargo.toml\", \"why\": \"cargo rebuilds it\"}}'")
    })?;
    parse_rule(&value, &world.places).map_err(|faults| anyhow!("{}", faults.join("; ")))
}

fn rule(world: &world::World, command: RuleCommand) -> Result<Value> {
    match command {
        RuleCommand::List => {
            let (gist, rules) = gh::load(&world.places)?;
            Ok(json!({"gist": gist.id, "rules": rules.iter().map(rule_json).collect::<Vec<_>>()}))
        }
        RuleCommand::Check { rule, all } => {
            let rule = given(&rule, world)?;
            let (gist, rules) = gh::load(&world.places)?;
            let existing = rules.iter().find(|r| r.id == rule.id);
            let plan = run::Run::new(world, false, all).rule(&rule)?;
            Ok(json!({
                "gist": gist.id,
                "rule": rule_json(&rule),
                "id_taken": existing.is_some(),
                "same_as_listed": existing == Some(&rule),
                "plan": plan,
            }))
        }
        RuleCommand::Add { rule } => {
            let rule = given(&rule, world)?;
            let (gist, mut rules) = gh::load(&world.places)?;
            let added = match rules.iter().find(|r| r.id == rule.id) {
                Some(r) if *r == rule => false,
                Some(_) => bail!(
                    "a different Rule has the id `{}`: pick another id, or change it with `diskreap rule set` or `diskreap rule rm` first",
                    rule.id
                ),
                None => {
                    rules.push(rule.clone());
                    gh::save(&gist, &rules)?;
                    true
                }
            };
            let plan = run::Run::new(world, false, false).rule(&rule)?;
            Ok(json!({"gist": gist.id, "added": added, "rule": rule_json(&rule), "plan": plan}))
        }
        RuleCommand::Set { id, min_age, why } => {
            if min_age.is_none() && why.is_none() {
                bail!("rule set changes nothing without --min-age or --why: give one");
            }
            let (gist, mut rules) = gh::load(&world.places)?;
            let known = ids(&rules);
            let rule = rules
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or_else(|| anyhow!("no Rule has the id `{id}`: the ids are {known}"))?;
            let before = rule.clone();
            if let Some(age) = min_age {
                rule.min_age = parse_age(&age).map_err(|e| anyhow!(e))?;
            }
            if let Some(why) = why {
                if why.trim().is_empty() {
                    bail!("--why is empty: say why what the Rule matches is safe to delete");
                }
                rule.why = why;
            }
            let after = rule.clone();
            let changed = after != before;
            if changed {
                gh::save(&gist, &rules)?;
            }
            Ok(json!({"gist": gist.id, "changed": changed, "rule": rule_json(&after)}))
        }
        RuleCommand::Rm { id } => {
            let (gist, mut rules) = gh::load(&world.places)?;
            let before = rules.len();
            rules.retain(|r| r.id != id);
            let removed = rules.len() != before;
            if removed {
                gh::save(&gist, &rules)?;
            }
            Ok(json!({"gist": gist.id, "removed": removed, "id": id, "rules": rules.len()}))
        }
    }
}

/// Every check, each with the line that fixes it when it fails.
fn doctor(world: &world::World) -> Value {
    let mut findings = Vec::new();
    let mut check = |name: &str, result: Result<String, String>, fix: &str| {
        findings.push(match result {
            Ok(says) => json!({"check": name, "ok": true, "says": says}),
            Err(says) => json!({"check": name, "ok": false, "says": says, "fix": fix}),
        });
    };
    let gh_ok = gh::status();
    let gh_good = gh_ok.is_ok();
    check("gh", gh_ok.map(|_| "logged in".into()), "brew install gh, then gh auth login");
    if gh_good {
        let gist = gh::open().map_err(|e| e.to_string()).and_then(|g| {
            let id = &g.id;
            parse_list(&g.text, &world.places)
                .map(|rules| format!("gist {id} holds {} Rules", rules.len()))
                .map_err(|f| format!("the Rule list in the gist {id} is wrong: {}", f.join("; ")))
        });
        check(
            "gist",
            gist,
            "diskreap init makes the gist; a wrong Rule list is fixed in the gist itself",
        );
    }
    check(
        "lsof",
        system::Open::read().map(|_| "lists open files".into()).map_err(|e| e.to_string()),
        "lsof comes with macOS: put /usr/sbin on PATH",
    );
    check(
        "git",
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .map_err(|e| format!("git cannot be run: {e}")),
        "xcode-select --install",
    );
    check(
        "free space",
        system::free(&world.places.home)
            .map(|b| format!("{b} bytes free"))
            .map_err(|e| e.to_string()),
        "df comes with macOS: put /bin on PATH",
    );
    let ok = findings.iter().all(|f| f["ok"] == true);
    json!({
        "ok": ok,
        "findings": findings,
        "places": Root::ALL
            .into_iter()
            .map(|root| (root.name().to_string(), json!(world.places.of(root))))
            .collect::<serde_json::Map<_, _>>(),
    })
}
