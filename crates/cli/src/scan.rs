//! `scan`: large directories no Rule covers, reported as they are found as
//! candidates for new Rules. It deletes nothing and adds no Rule.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use diskreap_judge::group;
use diskreap_rules::{Glob, Root, Rule, compile, path_matches};
use serde_json::{Value, json};

use crate::run::{Event, event};
use crate::system::{self, Apps};
use crate::walk;
use crate::world::World;

/// A directory this large, with no child this large, is reported.
pub const LARGE: u64 = 1 << 30;

/// The size `scan` reports from: [`LARGE`], or in a debug build what
/// `DISKREAP_LARGE` says, so tests need not write a gigabyte.
fn large_size() -> u64 {
    #[cfg(debug_assertions)]
    if let Some(n) = std::env::var("DISKREAP_LARGE").ok().and_then(|n| n.parse().ok()) {
        return n;
    }
    LARGE
}
/// How many of each kind the answer names.
pub const SHOWN: usize = 30;
/// An Application Support directory smaller than this is not reported.
pub const ORPHAN_MIN: u64 = 10 << 20;

/// Application Support directories macOS itself keeps.
const APPLE: [&str; 12] = [
    "AddressBook",
    "CallHistoryDB",
    "CallHistoryTransactions",
    "CloudDocs",
    "CrashReporter",
    "DiskImages",
    "FileProvider",
    "Knowledge",
    "MobileSync",
    "Caches",
    "iCloud",
    "SyncServices",
];

struct Covered<'a> {
    globs: Vec<Glob>,
    world: &'a World,
}

impl Covered<'_> {
    fn new<'a>(world: &'a World, rules: &[Rule]) -> Covered<'a> {
        let globs = rules.iter().filter_map(|r| compile(&r.glob, &world.places).ok()).collect();
        Covered { globs, world }
    }

    fn covers(&self, path: &str) -> bool {
        self.globs.iter().any(|g| path_matches(g, path))
    }
}

pub fn scan(world: &World, gist: &str, rules: &[Rule]) -> Result<Value> {
    let start = Instant::now();
    let covered = Covered::new(world, rules);
    event(Event::Start, json!({"scan": "large"}));
    let mut large = Vec::new();
    sizes(Path::new(&world.places.home), &covered, &mut large);
    large.sort_by_key(|l: &(String, u64)| std::cmp::Reverse(l.1));
    event(Event::Start, json!({"scan": "tmpdir"}));
    let groups = tmpdir(&covered);
    event(Event::Start, json!({"scan": "orphans"}));
    let orphans = orphans(&covered)?;
    let large: Vec<Value> =
        large.iter().take(SHOWN).map(|(p, b)| json!({"path": p, "bytes": b})).collect();
    Ok(json!({
        "gist": gist,
        "large": large,
        "tmpdir": groups,
        "orphans": orphans,
        "seconds": (start.elapsed().as_secs_f64() * 100.0).round() / 100.0,
        "next": "for each that is safe to delete, ask the person, then `diskreap rule check '<rule>'` and `diskreap rule add '<rule>'`",
    }))
}

/// The bytes under `dir` no Rule covers; each large directory with no large
/// child is reported as it is found.
fn sizes(dir: &Path, covered: &Covered, large: &mut Vec<(String, u64)>) -> u64 {
    let text = dir.to_string_lossy();
    if covered.covers(&text) {
        return 0;
    }
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return 0 };
    let mut total = meta.blocks().saturating_mul(512);
    if !meta.is_dir() || meta.dev() != covered.world.dev {
        return total;
    }
    let mut large_child = false;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let child = sizes(&entry.path(), covered, large);
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        large_child |= is_dir && child >= large_size();
        total = total.saturating_add(child);
    }
    if total >= large_size() && !large_child {
        event(Event::Large, json!({"path": text, "bytes": total}));
        large.push((text.into_owned(), total));
    }
    total
}

/// The temporary directory's entries, grouped by what made them, largest first.
fn tmpdir(covered: &Covered) -> Vec<Value> {
    let world = covered.world;
    let mut groups: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for entry in std::fs::read_dir(&world.places.tmpdir).into_iter().flatten().flatten() {
        let path = entry.path();
        let text = path.to_string_lossy();
        if covered.covers(&text) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let bytes = walk::tree(&path, world.uid, world.dev, None).bytes;
        let g = groups.entry(group(&name).to_string()).or_default();
        g.0 += 1;
        g.1 = g.1.saturating_add(bytes);
    }
    let mut out: Vec<(String, u64, u64)> =
        groups.into_iter().map(|(k, (n, b))| (k, n, b)).collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.2));
    out.into_iter()
        .take(SHOWN)
        .map(|(prefix, entries, bytes)| {
            let v = json!({"group": prefix, "glob": format!("{}/{prefix}*", Root::Tmpdir.name()), "entries": entries, "bytes": bytes});
            event(Event::Tmpdir, json!({"group": prefix, "entries": entries, "bytes": bytes}));
            v
        })
        .collect()
}

/// Application Support directories no installed app seems to own.
fn orphans(covered: &Covered) -> Result<Vec<Value>> {
    let world = covered.world;
    let apps = Apps::read(&world.places.home);
    let programs = system::running()?;
    let support = Path::new(&world.places.home).join("Library/Application Support");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&support).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let text = path.to_string_lossy().into_owned();
        if name.starts_with("com.apple.")
            || APPLE.contains(&name.as_str())
            || !path.is_dir()
            || covered.covers(&text)
            || apps.resembles(&name)
            || system::is_running(&programs, &name)
        {
            continue;
        }
        let bytes = walk::tree(&path, world.uid, world.dev, None).bytes;
        if bytes < ORPHAN_MIN {
            continue;
        }
        event(Event::Orphan, json!({"path": text, "bytes": bytes}));
        out.push(json!({
            "path": text,
            "bytes": bytes,
            "why": "no installed app's name or bundle id resembles it: ask the person which app it was, and whether it is uninstalled",
            "rule": {
                "id": format!("orphan-{}", slug(&name)),
                "glob": format!("~/Library/Application Support/{name}"),
                "unless_installed": [name],
                "min_age": "1d",
                "why": "<what app it was, and that it is uninstalled>",
            },
        }));
    }
    out.sort_by(|a, b| b["bytes"].as_u64().cmp(&a["bytes"].as_u64()));
    out.truncate(SHOWN);
    Ok(out)
}

fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}
