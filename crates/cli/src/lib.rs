//! The effect layer: reads the world, asks the pure crates what to make of
//! it, acts, and prints the answer. Every command prints one JSON object on
//! stdout and, while it walks, one JSON event a line on stderr; `--help`,
//! `-h` and `--version` print text.

use std::ffi::OsString;
use std::io::Read;
use std::process::ExitCode;

use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand};
use diskreap_rules::{FILE, Rule, parse_age, parse_list, parse_rule, rule_json};
use serde_json::{Value, json};

mod gh;
mod par;
mod run;
mod scan;
mod system;
mod walk;
mod world;

/// What this binary was built from, as `--version` says it: stamped by `build.rs`.
pub const VERSION: &str = env!("DISKREAP_VERSION");

/// The guide `--help` prints, and the one screen `-h` prints.
pub const GUIDE: &str = include_str!("help.txt");
pub const SCREEN: &str = include_str!("short.txt");

/// Each subcommand's help, by the words that name it.
pub const HELP: [(&str, &str); 10] = [
    ("init", include_str!("help/init.txt")),
    ("doctor", include_str!("help/doctor.txt")),
    ("plan", include_str!("help/plan.txt")),
    ("clean", include_str!("help/clean.txt")),
    ("scan", include_str!("help/scan.txt")),
    ("rule list", include_str!("help/rule-list.txt")),
    ("rule check", include_str!("help/rule-check.txt")),
    ("rule add", include_str!("help/rule-add.txt")),
    ("rule set", include_str!("help/rule-set.txt")),
    ("rule rm", include_str!("help/rule-rm.txt")),
];

#[derive(Parser)]
#[command(name = "diskreap", version = VERSION, disable_help_flag = true, disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Make the private gist that keeps the Rule list, if there is none
    Init,
    /// Check everything diskreap needs, changing nothing
    Doctor,
    /// Say what clean would delete and what it would hold, changing nothing
    Plan {
        /// Only this Rule, by id
        #[arg(long)]
        rule: Option<String>,
        /// Name every Match, not only the largest ten of each Rule
        #[arg(long)]
        all: bool,
    },
    /// Delete every Match nothing holds, each as it is found
    Clean {
        /// Only this Rule, by id
        #[arg(long)]
        rule: Option<String>,
        /// Name every Match, not only the largest ten of each Rule
        #[arg(long)]
        all: bool,
    },
    /// Find large directories no Rule covers, deleting nothing
    Scan,
    /// Read and change the Rule list
    Rule {
        #[command(subcommand)]
        command: RuleCommand,
    },
}

#[derive(Subcommand)]
enum RuleCommand {
    /// Print the Rule list
    List,
    /// Say what a Rule would match and hold, without adding it
    Check {
        /// The Rule as JSON, or `-` to read it from stdin
        rule: String,
        /// Name every Match, not only the largest ten
        #[arg(long)]
        all: bool,
    },
    /// Add a Rule a person agreed to
    Add {
        /// The Rule as JSON, or `-` to read it from stdin
        rule: String,
    },
    /// Change a Rule's Minimum age or why
    Set {
        id: String,
        #[arg(long)]
        min_age: Option<String>,
        #[arg(long)]
        why: Option<String>,
    },
    /// Remove a Rule
    Rm { id: String },
}

/// Runs diskreap with these arguments, the program's name first.
pub fn main(args: Vec<OsString>) -> ExitCode {
    if let Some(text) = help(&args) {
        print!("{text}");
        return ExitCode::SUCCESS;
    }
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayVersion => {
            println!("diskreap {VERSION}");
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
    match command(cli.command) {
        Ok(answer) => {
            println!("{answer}");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&format!("{e:#}")),
    }
}

fn fail(message: &str) -> ExitCode {
    println!("{}", json!({ "error": message }));
    ExitCode::FAILURE
}

/// The help asked for, if any: the guide or the one screen with no
/// subcommand, the subcommand's own help after one.
fn help(args: &[OsString]) -> Option<&'static str> {
    let words: Vec<String> =
        args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let flag = words.iter().find(|w| *w == "--help" || *w == "-h")?;
    let named: Vec<&str> =
        words.iter().take_while(|w| !w.starts_with('-')).map(String::as_str).collect();
    for n in (1..=named.len().min(2)).rev() {
        let key = named[..n].join(" ");
        if let Some((_, text)) = HELP.iter().find(|(k, _)| *k == key) {
            return Some(text);
        }
    }
    if named.first() == Some(&"rule") {
        return Some(include_str!("help/rule.txt"));
    }
    Some(if flag == "--help" { GUIDE } else { SCREEN })
}

fn command(command: Command) -> Result<Value> {
    let world = world::read()?;
    match command {
        Command::Init => init(),
        Command::Doctor => Ok(doctor(&world)),
        Command::Plan { rule, all } => plan(&world, rule.as_deref(), false, all),
        Command::Clean { rule, all } => plan(&world, rule.as_deref(), true, all),
        Command::Scan => {
            let (gist, rules) = gh::load(&world.places)?;
            scan::scan(&world, &gist.id, &rules)
        }
        Command::Rule { command } => rule(&world, command),
    }
}

fn init() -> Result<Value> {
    let ids = gh::find()?;
    match ids.as_slice() {
        [] => {
            let id = gh::create()?;
            Ok(json!({"gist": id, "created": true, "rules": 0, "file": FILE}))
        }
        [id] => {
            let gist = gh::read(id)?;
            let count = serde_json::from_str::<Value>(&gist.text)
                .ok()
                .and_then(|v| v["rules"].as_array().map(Vec::len));
            Ok(json!({"gist": id, "created": false, "rules": count, "file": FILE}))
        }
        _ => bail!(
            "several gists hold {FILE} ({}): keep one, delete the others with `gh gist delete <id>`",
            ids.join(", ")
        ),
    }
}

fn plan(world: &world::World, only: Option<&str>, delete: bool, all: bool) -> Result<Value> {
    let (gist, rules) = gh::load(&world.places)?;
    let chosen: Vec<Rule> =
        match only {
            None => rules,
            Some(id) => vec![rules.iter().find(|r| r.id == id).cloned().ok_or_else(|| {
                anyhow!("no Rule has the id `{id}`: the ids are {}", ids(&rules))
            })?],
        };
    run::rules(world, &gist.id, &chosen, delete, all)
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
        let gist = gh::the_gist().map_err(|e| e.to_string()).and_then(|id| {
            let g = gh::read(&id).map_err(|e| e.to_string())?;
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
        "places": {
            "~": world.places.home,
            "$TMPDIR": world.places.tmpdir,
            "$VARFOLDER": world.places.var_folder,
        },
    })
}
