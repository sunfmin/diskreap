//! Judging each Rule's Matches as they are found — and, for `clean`,
//! deleting each one that goes right then — with an event on stderr for each.

use std::io::Write;
use std::time::Instant;

use anyhow::Result;
use diskreap_judge::{Facts, Judged, Verdict, judge, judged_json, summary};
use diskreap_rules::{Rule, compile, floor};
use serde_json::{Value, json};

use crate::system::{self, Apps, Open};
use crate::walk;
use crate::world::World;

/// Writes one event as a line of JSON on stderr, as it happens.
pub fn event(value: Value) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{value}");
    let _ = err.flush();
}

/// What a run reads at most once: the open files (read again when stale),
/// the programs running and the apps installed.
pub struct Run<'a> {
    pub world: &'a World,
    pub delete: bool,
    pub all: bool,
    open: Option<Open>,
    programs: Option<Vec<String>>,
    apps: Option<Apps>,
}

impl<'a> Run<'a> {
    pub fn new(world: &'a World, delete: bool, all: bool) -> Run<'a> {
        Run { world, delete, all, open: None, programs: None, apps: None }
    }

    fn open(&mut self) -> Result<&Open> {
        match &mut self.open {
            Some(open) => open.fresh()?,
            None => self.open = Some(Open::read()?),
        }
        Ok(self.open.as_ref().expect("read above"))
    }

    /// One Rule's run: every Match judged as it is found, and gone at once
    /// when deleting and nothing holds it.
    pub fn rule(&mut self, rule: &Rule) -> Result<Value> {
        let start = Instant::now();
        let world = self.world;
        let places = &world.places;
        let glob = match compile(&rule.glob, places) {
            Ok(g) => g,
            Err(fault) => return Ok(json!({"glob": rule.glob, "error": fault})),
        };
        let running: Vec<String> = if rule.unless_running.is_empty() {
            Vec::new()
        } else {
            let programs = match &self.programs {
                Some(p) => p.clone(),
                None => {
                    let p = system::running()?;
                    self.programs = Some(p.clone());
                    p
                }
            };
            rule.unless_running
                .iter()
                .filter(|n| system::is_running(&programs, n))
                .cloned()
                .collect()
        };
        let installed: Vec<String> = if rule.unless_installed.is_empty() {
            Vec::new()
        } else {
            let apps = self.apps.get_or_insert_with(|| Apps::read(&places.home));
            rule.unless_installed.iter().filter(|n| apps.installed(n)).cloned().collect()
        };
        let (uid, dev, now) = (self.world.uid, self.world.dev, self.world.now);
        let mut judged: Vec<Judged> = Vec::new();
        let mut deleted: u64 = 0;
        let mut deleted_bytes: u64 = 0;
        let mut failed: Vec<Value> = Vec::new();
        let mut fault: Option<anyhow::Error> = None;
        let mut found = |path: std::path::PathBuf| {
            if fault.is_some() {
                return;
            }
            let text = path.to_string_lossy().into_owned();
            let tree = walk::tree(&path, uid, dev);
            // Only a Match the floor lets be judged is worth asking git and lsof about.
            let judged_at_all = floor(&text, places).is_none();
            let in_use = judged_at_all
                && match self.open() {
                    Ok(open) => open.holds(&text),
                    Err(e) => {
                        fault = Some(e);
                        return;
                    }
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
                git: if judged_at_all { system::git(&path) } else { diskreap_judge::Git::Outside },
                running: running.clone(),
                installed: installed.clone(),
            };
            let verdict = judge(&facts, rule.min_age, now, places);
            let one = Judged { path: text.clone(), bytes: tree.bytes, verdict };
            let mut said = judged_json(&one);
            said["event"] = json!("match");
            said["rule"] = json!(rule.id);
            if self.delete && one.verdict == Verdict::Delete {
                match walk::delete(&path) {
                    Ok(()) => {
                        deleted = deleted.saturating_add(1);
                        deleted_bytes = deleted_bytes.saturating_add(tree.bytes);
                        said["event"] = json!("deleted");
                    }
                    Err(e) => {
                        let f = json!({"path": text, "error": e.to_string()});
                        failed.push(f);
                        said["event"] = json!("failed");
                        said["error"] = json!(e.to_string());
                    }
                }
            }
            event(said);
            judged.push(one);
        };
        walk::matches(&glob, rule.marker.as_deref(), places, dev, &mut found);
        if let Some(e) = fault {
            return Err(e);
        }
        let mut answer = summary(&judged, self.all);
        answer["glob"] = json!(rule.glob);
        answer["seconds"] = json!((start.elapsed().as_secs_f64() * 100.0).round() / 100.0);
        if self.delete {
            answer["deleted"] = json!(deleted);
            answer["deleted_bytes"] = json!(deleted_bytes);
            answer["failed"] = json!(failed);
        }
        event(
            json!({"event": "rule", "rule": rule.id, "matched": answer["matched"], "deletable": answer["deletable"], "seconds": answer["seconds"]}),
        );
        Ok(answer)
    }
}

/// `plan` and `clean`: every Rule, or the one named, run in turn.
pub fn rules(world: &World, gist: &str, rules: &[Rule], delete: bool, all: bool) -> Result<Value> {
    let start = Instant::now();
    let free_before = if delete { Some(system::free(&world.places.home)?) } else { None };
    let mut run = Run::new(world, delete, all);
    let mut per = serde_json::Map::new();
    let (mut matched, mut deletable, mut deletable_bytes, mut deleted, mut deleted_bytes) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut failed = Vec::new();
    for rule in rules {
        event(json!({"event": "start", "rule": rule.id, "glob": rule.glob}));
        let answer = run.rule(rule)?;
        matched += answer["matched"].as_u64().unwrap_or(0);
        deletable += answer["deletable"].as_u64().unwrap_or(0);
        deletable_bytes += answer["deletable_bytes"].as_u64().unwrap_or(0);
        deleted += answer["deleted"].as_u64().unwrap_or(0);
        deleted_bytes += answer["deleted_bytes"].as_u64().unwrap_or(0);
        if let Some(f) = answer["failed"].as_array() {
            failed.extend(f.iter().cloned());
        }
        per.insert(rule.id.clone(), answer);
    }
    let mut answer = json!({
        "gist": gist,
        "rules": per,
        "matched": matched,
        "deletable": deletable,
        "deletable_bytes": deletable_bytes,
        "note": "bytes are allocated blocks: a file APFS cloned counts once in each copy, so they can add up to more than is freed",
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
    answer["seconds"] = json!((start.elapsed().as_secs_f64() * 100.0).round() / 100.0);
    Ok(answer)
}
