//! Judging each Rule's Matches as they are found — and, for `clean`,
//! deleting each one that goes right then — with an event on stderr for each.
//! Rules that search any depth from the same directory share one walk.

use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use agent_help::Table;
use anyhow::Result;
use diskreap_judge::{Facts, Git, Judged, Verdict, judge, judged_json, summary};
use diskreap_rules::{Glob, Rule, compile, floor};
use serde_json::{Value, json};

use crate::system::{self, Apps, Open};
use crate::walk;
use crate::world::World;

/// What an event on stderr says happened, as its `"event"` key names it.
#[derive(Clone, Copy, agent_help::Help)]
pub enum Event {
    /// A Rule's walk starts (`rule`, `glob`), or a part of scan does (`scan`).
    Start,
    /// A Match was judged: `rule`, `path`, `bytes`, `verdict`, and `holds` when held.
    Match,
    /// clean deleted a Match: as `match` says it.
    Deleted,
    /// clean could not delete a Match: as `match` says it, and `error`.
    Failed,
    /// A Rule is done: `rule`, `matched`, `deletable`, `seconds`.
    Rule,
    /// scan found a large directory no Rule covers: `path`, `bytes`.
    Large,
    /// scan grouped $TMPDIR's entries made by one program: `group`, `entries`, `bytes`.
    Tmpdir,
    /// scan found an Application Support directory no installed app resembles: `path`, `bytes`.
    Orphan,
}

/// Writes one event as a line of JSON on stderr, as it happens: `said`, an
/// object, with `"event"` naming what happened.
pub fn event(happened: Event, mut said: Value) {
    said["event"] = json!(happened.entry().name);
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{said}");
    let _ = err.flush();
}

fn seconds(start: Instant) -> f64 {
    (start.elapsed().as_secs_f64() * 100.0).round() / 100.0
}

/// What a run reads at most once: the open files (read again when stale),
/// the programs running and the apps installed.
pub struct Run<'a> {
    pub world: &'a World,
    pub delete: bool,
    pub all: bool,
    /// The paths to judge, in place of walking for every Match; none to walk.
    pub only: Vec<PathBuf>,
    /// Those of `only` no Rule matches.
    pub unmatched: Vec<PathBuf>,
    open: Option<Open>,
    programs: Option<Vec<String>>,
    apps: Option<Apps>,
}

/// One Rule's part of a run.
struct Lane<'r> {
    rule: &'r Rule,
    glob: Result<Glob, String>,
    running: Vec<String>,
    installed: Vec<String>,
    /// Anything changed after this is within the Minimum age.
    cutoff: i64,
    judged: Vec<Judged>,
    deleted: u64,
    deleted_bytes: u64,
    failed: Vec<Value>,
}

impl<'a> Run<'a> {
    pub fn new(world: &'a World, delete: bool, all: bool) -> Run<'a> {
        Run {
            world,
            delete,
            all,
            only: Vec::new(),
            unmatched: Vec::new(),
            open: None,
            programs: None,
            apps: None,
        }
    }

    /// One Rule's run.
    pub fn rule(&mut self, rule: &Rule) -> Result<Value> {
        let mut answers = self.rules(std::slice::from_ref(rule))?;
        Ok(answers.pop().map(|(_, a)| a).unwrap_or_default())
    }

    fn lane<'r>(&mut self, rule: &'r Rule) -> Result<Lane<'r>> {
        let world = self.world;
        let running = if rule.unless_running.is_empty() {
            Vec::new()
        } else {
            if self.programs.is_none() {
                self.programs = Some(system::running()?);
            }
            let programs = self.programs.as_deref().unwrap_or_default();
            rule.unless_running
                .iter()
                .filter(|n| system::is_running(programs, n))
                .cloned()
                .collect()
        };
        let installed = if rule.unless_installed.is_empty() {
            Vec::new()
        } else {
            let apps = self.apps.get_or_insert_with(|| Apps::read(&world.places.home));
            rule.unless_installed.iter().filter(|n| apps.installed(n)).cloned().collect()
        };
        let age = i64::try_from(rule.min_age.secs()).unwrap_or(i64::MAX);
        Ok(Lane {
            rule,
            glob: compile(&rule.glob, &world.places),
            running,
            installed,
            cutoff: world.now.saturating_sub(age),
            judged: Vec::new(),
            deleted: 0,
            deleted_bytes: 0,
            failed: Vec::new(),
        })
    }

    /// Several Rules' runs, sharing walks: every Match judged as it is found,
    /// and gone at once when deleting and nothing holds it. Each Rule's
    /// answer, in the order given.
    pub fn rules(&mut self, rules: &[Rule]) -> Result<Vec<(String, Value)>> {
        let start = Instant::now();
        let world = self.world;
        let places = &world.places;
        let mut lanes = Vec::new();
        for rule in rules {
            event(Event::Start, json!({"rule": rule.id, "glob": rule.glob}));
            lanes.push(self.lane(rule)?);
        }
        let owned: Vec<(usize, Glob, &[String])> = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.glob.clone().ok().map(|g| (i, g, l.rule.marker.as_slice())))
            .collect();
        let globs: Vec<walk::Wanted> = owned.iter().map(|(i, g, m)| (*i, g, *m)).collect();
        let (uid, dev, now, delete) = (world.uid, world.dev, world.now, self.delete);
        let mut open = self.open.take();
        let mut fault: Option<anyhow::Error> = None;
        let mut found = |i: usize, path: PathBuf| {
            if fault.is_some() {
                return;
            }
            let lane = &mut lanes[i];
            let text = path.to_string_lossy().into_owned();
            let tree = walk::tree(&path, uid, dev, Some(lane.cutoff));
            // Only a Match the floor lets be judged is worth asking git and lsof about.
            let judged_at_all = floor(&text, places).is_none();
            let in_use = if judged_at_all {
                let read = match open.as_mut() {
                    Some(o) => o.fresh(),
                    None => Open::read().map(|o| open = Some(o)),
                };
                if let Err(e) = read {
                    fault = Some(e);
                    return;
                }
                open.as_ref().is_some_and(|o| o.holds(&text))
            } else {
                false
            };
            let facts = Facts {
                path: text.clone(),
                bytes: tree.bytes,
                newest: tree.newest,
                foreign: tree.foreign,
                unreadable: tree.unreadable,
                holds_repo: tree.holds_repo,
                other_volume: tree.other_volume,
                in_use,
                git: if judged_at_all { system::git(&path) } else { Git::Outside },
                running: lane.running.clone(),
                installed: lane.installed.clone(),
            };
            let verdict = judge(&facts, lane.rule.min_age, now, places);
            let one =
                Judged { path: text.clone(), bytes: tree.bytes, complete: tree.complete, verdict };
            let mut said = judged_json(&one);
            said["rule"] = json!(lane.rule.id);
            let mut happened = Event::Match;
            // A walk cut short found a Hold; it never deletes, whatever the Verdict.
            if delete && one.verdict == Verdict::Delete && tree.complete {
                match walk::delete(&path) {
                    Ok(()) => {
                        lane.deleted = lane.deleted.saturating_add(1);
                        lane.deleted_bytes = lane.deleted_bytes.saturating_add(tree.bytes);
                        happened = Event::Deleted;
                    }
                    Err(e) => {
                        lane.failed.push(json!({"path": text, "error": e.to_string()}));
                        happened = Event::Failed;
                        said["error"] = json!(e.to_string());
                    }
                }
            }
            event(happened, said);
            lane.judged.push(one);
        };
        if self.only.is_empty() {
            walk::matches(&globs, places, dev, &mut found);
        }
        for path in &self.only {
            let mut hit = false;
            walk::at(&globs, places, dev, path, &mut |i, p| {
                hit = true;
                found(i, p);
            });
            if !hit {
                self.unmatched.push(path.clone());
            }
        }
        self.open = open;
        if let Some(e) = fault {
            return Err(e);
        }
        let took = seconds(start);
        Ok(lanes
            .into_iter()
            .map(|lane| {
                let answer = match &lane.glob {
                    Err(fault) => json!({"glob": lane.rule.glob, "error": fault}),
                    Ok(_) => {
                        let mut a = summary(&lane.judged, self.all);
                        a["glob"] = json!(lane.rule.glob);
                        a["seconds"] = json!(took);
                        if self.delete {
                            a["deleted"] = json!(lane.deleted);
                            a["deleted_bytes"] = json!(lane.deleted_bytes);
                            a["failed"] = json!(lane.failed);
                        }
                        a
                    }
                };
                event(Event::Rule, json!({"rule": lane.rule.id, "matched": answer["matched"], "deletable": answer["deletable"], "seconds": took}));
                (lane.rule.id.clone(), answer)
            })
            .collect())
    }
}

/// `plan` and `clean`: every Rule, or the one named.
pub fn rules(
    world: &World,
    gist: &str,
    rules: &[Rule],
    only: Vec<PathBuf>,
    delete: bool,
    all: bool,
) -> Result<Value> {
    let start = Instant::now();
    let free_before = if delete { Some(system::free(&world.places.home)?) } else { None };
    let mut run = Run::new(world, delete, all);
    run.only = only.clone();
    let mut per = serde_json::Map::new();
    let (mut matched, mut deletable, mut deletable_bytes, mut deleted, mut deleted_bytes) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut failed = Vec::new();
    for (id, answer) in run.rules(rules)? {
        matched += answer["matched"].as_u64().unwrap_or(0);
        deletable += answer["deletable"].as_u64().unwrap_or(0);
        deletable_bytes += answer["deletable_bytes"].as_u64().unwrap_or(0);
        deleted += answer["deleted"].as_u64().unwrap_or(0);
        deleted_bytes += answer["deleted_bytes"].as_u64().unwrap_or(0);
        if let Some(f) = answer["failed"].as_array() {
            failed.extend(f.iter().cloned());
        }
        per.insert(id, answer);
    }
    let mut answer = json!({
        "gist": gist,
        "rules": per,
        "matched": matched,
        "deletable": deletable,
        "deletable_bytes": deletable_bytes,
        "note": "bytes are allocated blocks: a file APFS cloned counts once in each copy, so they can add up to more than is freed; a held Match with bytes_at_least was not read to the end",
    });
    if let Some(before) = free_before {
        let after = system::free(&world.places.home)?;
        answer["deleted"] = json!(deleted);
        answer["deleted_bytes"] = json!(deleted_bytes);
        answer["failed"] = json!(failed);
        answer["free_before"] = json!(before);
        answer["free_after"] = json!(after);
        answer["freed_bytes"] = json!(i128::from(after) - i128::from(before));
    }
    if !only.is_empty() {
        answer["paths"] = json!(only);
        answer["unmatched"] = json!(run.unmatched);
        if !run.unmatched.is_empty() {
            answer["why"] = json!(
                "no Rule matches the unmatched paths: its glob does not reach them, its marker is not beside them, or they are on another volume; `diskreap rule list` shows the globs"
            );
        }
    }
    answer["seconds"] = json!(seconds(start));
    Ok(answer)
}
