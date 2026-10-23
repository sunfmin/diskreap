//! Whether a Match goes or is held, and what a run says of each Rule. The
//! effect crate reads what is true of a Match into its [`Facts`]; [`judge`]
//! names every Hold, and a [`Verdict::Delete`] comes only from a list of Holds
//! that is empty: Flux checks that `decide` is the one way to it.

use diskreap_rules::{Age, Places, floor};
extern crate flux_alloc;
extern crate flux_core;

use flux_rs::attrs::*;
use serde_json::{Map, Value};

/// Whether the Match is inside a git work tree, and if so whether git ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Git {
    Outside,
    Ignored,
    /// Git tracks it, or something in it, or would add it.
    NotIgnored,
}

/// What is true of a Match on this machine, as the effect crate read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub path: String,
    /// Disk space its entries take, counted as allocated blocks: a file APFS
    /// cloned is counted in each copy.
    pub bytes: u64,
    /// Seconds since 1970 of the latest it, or anything in it, was changed or
    /// used: the latest of each entry's modification and status change and,
    /// for a file, the latest it may have been read (see the effect crate).
    /// A directory's reading is not use: listing it is how a walk reads it.
    pub newest: i64,
    /// Something in it is owned by another user.
    pub foreign: bool,
    /// Something in it could not be read.
    pub unreadable: bool,
    /// It holds a `.git`: a repository whose work may not be pushed.
    pub holds_repo: bool,
    /// Something in it is on another volume than the home directory.
    pub other_volume: bool,
    /// A process has a file in it open, or works in it.
    pub in_use: bool,
    pub git: Git,
    /// The programs of the Rule's `unless_running` that are running.
    pub running: Vec<String>,
    /// The apps of the Rule's `unless_installed` that are installed.
    pub installed: Vec<String>,
}

/// A reason a Match stays this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    Floor(&'static str),
    /// Changed within the Rule's Minimum age; the seconds left to wait.
    Recent(u64),
    InUse,
    Running(String),
    Installed(String),
    Foreign,
    Unreadable,
    NotIgnored,
    HoldsRepo,
    OtherVolume,
}

impl Hold {
    /// The Hold's kind, as a run counts it.
    pub fn key(&self) -> &'static str {
        match self {
            Hold::Floor(_) => "floor",
            Hold::Recent(_) => "recent",
            Hold::InUse => "in_use",
            Hold::Running(_) => "running",
            Hold::Installed(_) => "installed",
            Hold::Foreign => "not_yours",
            Hold::Unreadable => "unreadable",
            Hold::NotIgnored => "git_tracked",
            Hold::HoldsRepo => "holds_repo",
            Hold::OtherVolume => "other_volume",
        }
    }

    /// The Hold said in a sentence.
    pub fn text(&self) -> String {
        match self {
            Hold::Floor(why) => format!("the floor keeps it: it {why}"),
            Hold::Recent(left) => {
                format!("changed or used too recently: it may go in {}", wait_text(*left))
            }
            Hold::InUse => "a process has a file in it open, or works in it".into(),
            Hold::Running(name) => format!("{name} is running"),
            Hold::Installed(name) => format!("{name} is installed"),
            Hold::Foreign => "something in it belongs to another user".into(),
            Hold::Unreadable => "something in it could not be read".into(),
            Hold::NotIgnored => "it is in a git work tree and git does not ignore it".into(),
            Hold::HoldsRepo => "it holds a git repository".into(),
            Hold::OtherVolume => "something in it is on another volume".into(),
        }
    }
}

fn wait_text(secs: u64) -> String {
    if secs >= 86_400 {
        format!("{}d", secs.div_ceil(86_400))
    } else if secs >= 3_600 {
        format!("{}h", secs.div_ceil(3_600))
    } else {
        format!("{}m", secs.div_ceil(60))
    }
}

/// The Holds found so far, and whether there are none.
#[refined_by(clear: bool)]
pub struct Holds {
    #[field(bool[clear])]
    clear: bool,
    list: Vec<Hold>,
}

impl Holds {
    #[spec(fn() -> Holds[true])]
    fn none() -> Holds {
        Holds { clear: true, list: Vec::new() }
    }

    #[spec(fn(Holds, Hold) -> Holds[false])]
    fn with(self, hold: Hold) -> Holds {
        let mut list = self.list;
        list.push(hold);
        Holds { clear: false, list }
    }
}

/// Whether a Match goes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[refined_by(delete: bool)]
pub enum Verdict {
    #[variant(Verdict[true])]
    Delete,
    #[variant((Vec<Hold>) -> Verdict[false])]
    Hold(Vec<Hold>),
}

/// The one way to a Verdict: it deletes exactly when nothing holds.
#[spec(fn(Holds[@clear]) -> Verdict[clear])]
fn decide(holds: Holds) -> Verdict {
    if holds.clear { Verdict::Delete } else { Verdict::Hold(holds.list) }
}

/// Seconds from the latest change or use to now; none for one in the future.
#[spec(fn(now: i64, newest: i64) -> u64{v: now <= newest => v == 0})]
pub fn waited(now: i64, newest: i64) -> u64 {
    if now > newest { now.abs_diff(newest) } else { 0 }
}

/// The seconds still to wait before a Match that has waited `waited` is old
/// enough, never more than the Minimum age; none when it is old enough.
#[spec(fn(waited: u64, min: Age[@m]) -> Option<u64{v: 0 < v && v <= m}>)]
pub fn left(waited: u64, min: Age) -> Option<u64> {
    let m = min.secs();
    if waited < m { Some(m - waited) } else { None }
}

/// Every Hold on a Match, and so whether it goes.
pub fn judge(facts: &Facts, min_age: Age, now: i64, places: &Places) -> Verdict {
    let mut holds = Holds::none();
    if let Some(why) = floor(&facts.path, places) {
        holds = holds.with(Hold::Floor(why));
    }
    if let Some(wait) = left(waited(now, facts.newest), min_age) {
        holds = holds.with(Hold::Recent(wait));
    }
    if facts.in_use {
        holds = holds.with(Hold::InUse);
    }
    for name in &facts.running {
        holds = holds.with(Hold::Running(name.clone()));
    }
    for name in &facts.installed {
        holds = holds.with(Hold::Installed(name.clone()));
    }
    if facts.foreign {
        holds = holds.with(Hold::Foreign);
    }
    if facts.unreadable {
        holds = holds.with(Hold::Unreadable);
    }
    if facts.git == Git::NotIgnored {
        holds = holds.with(Hold::NotIgnored);
    }
    if facts.holds_repo {
        holds = holds.with(Hold::HoldsRepo);
    }
    if facts.other_volume {
        holds = holds.with(Hold::OtherVolume);
    }
    decide(holds)
}

/// A Match with its size and Verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    pub path: String,
    pub bytes: u64,
    /// Whether `bytes` counts all of it; a walk stops early at the first Hold.
    pub complete: bool,
    pub verdict: Verdict,
}

/// One Match as an answer names it.
pub fn judged_json(j: &Judged) -> Value {
    let mut object = Map::new();
    object.insert("path".into(), Value::String(j.path.clone()));
    object.insert("bytes".into(), Value::from(j.bytes));
    if !j.complete {
        object.insert("bytes_at_least".into(), Value::Bool(true));
    }
    match &j.verdict {
        Verdict::Delete => {
            object.insert("verdict".into(), Value::from("delete"));
        }
        Verdict::Hold(holds) => {
            object.insert("verdict".into(), Value::from("hold"));
            let said = holds.iter().map(|h| Value::String(h.text())).collect();
            object.insert("holds".into(), Value::Array(said));
        }
    }
    Value::Object(object)
}

/// How many of the largest Matches a summary names.
pub const LARGEST: usize = 10;

/// What a run says of one Rule's Matches: how many there are, how many go
/// and how much they take, how many stay and why, and the largest.
pub fn summary(judged: &[Judged], all: bool) -> Value {
    let mut deletable: u64 = 0;
    let mut deletable_bytes: u64 = 0;
    let mut held: u64 = 0;
    let mut held_bytes: u64 = 0;
    let mut by_hold = Map::new();
    for j in judged {
        match &j.verdict {
            Verdict::Delete => {
                deletable = deletable.saturating_add(1);
                deletable_bytes = deletable_bytes.saturating_add(j.bytes);
            }
            Verdict::Hold(holds) => {
                held = held.saturating_add(1);
                held_bytes = held_bytes.saturating_add(j.bytes);
                for hold in holds {
                    let n = by_hold.get(hold.key()).and_then(Value::as_u64).unwrap_or(0);
                    by_hold.insert(hold.key().into(), Value::from(n.saturating_add(1)));
                }
            }
        }
    }
    let mut sorted: Vec<&Judged> = judged.iter().collect();
    sorted.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    let shown: Vec<Value> =
        sorted.into_iter().take(if all { usize::MAX } else { LARGEST }).map(judged_json).collect();
    let mut object = Map::new();
    object.insert("matched".into(), Value::from(judged.len()));
    object.insert("deletable".into(), Value::from(deletable));
    object.insert("deletable_bytes".into(), Value::from(deletable_bytes));
    object.insert("held".into(), Value::from(held));
    object.insert("held_bytes".into(), Value::from(held_bytes));
    object.insert("holds".into(), Value::Object(by_hold));
    object.insert(if all { "matches" } else { "largest" }.into(), Value::Array(shown));
    Value::Object(object)
}

/// The name a temporary entry is grouped under by `scan`: its name less the
/// numbers and random-looking parts a program added to make it unique, so
/// `bk-catalog-10096-0` and `bk-catalog-11062-12` are both `bk-catalog`.
pub fn group(name: &str) -> &str {
    let mut rest = name;
    loop {
        let cut = match rest.rfind(['-', '.', '_']) {
            Some(i) if i > 0 => i,
            _ => break,
        };
        let (head, tail) = rest.split_at(cut);
        let token = tail.get(1..).unwrap_or_default();
        if !unique_part(token) {
            break;
        }
        rest = head;
    }
    // Digits stuck to the end, as in `go-build3562170816`.
    let stem = rest.trim_end_matches(|c: char| c.is_ascii_digit());
    if !stem.is_empty() && rest.len().saturating_sub(stem.len()) >= 3 { stem } else { rest }
}

/// A part a program adds to make a name unique: hex with a digit in it, or six
/// or more letters and digits that switch between them twice or mix cases,
/// not a word with a number on one end.
fn unique_part(token: &str) -> bool {
    let digit = token.chars().any(|c| c.is_ascii_digit());
    let hex = token.chars().all(|c| c.is_ascii_hexdigit());
    let alnum = token.chars().all(|c| c.is_ascii_alphanumeric());
    let turns = token
        .chars()
        .zip(token.chars().skip(1))
        .filter(|(a, b)| a.is_ascii_digit() != b.is_ascii_digit())
        .count();
    let cases = token.chars().any(|c| c.is_ascii_uppercase())
        && token.chars().any(|c| c.is_ascii_lowercase());
    let mixed = turns >= 2 || cases;
    !token.is_empty() && digit && (hex || (alnum && mixed && token.len() >= 6))
}

#[cfg(test)]
mod tests;

/// What the refinements forbid, each written the way a careless caller would:
/// Flux must refuse every one.
#[cfg(flux)]
#[allow(dead_code)]
mod misuse {
    use super::*;

    /// A Match with a Hold cannot come out as deleted.
    #[should_fail]
    #[spec(fn(Holds[false]) -> Verdict[true])]
    fn held_yet_deleted(holds: Holds) -> Verdict {
        decide(holds)
    }

    /// Adding a Hold leaves the list not clear.
    #[should_fail]
    #[spec(fn() -> Verdict[true])]
    fn hold_forgotten() -> Verdict {
        decide(Holds::none().with(Hold::InUse))
    }

    /// A Match that has not waited at all is never old enough.
    #[should_fail]
    #[spec(fn(Age) -> Option<u64>[false])]
    fn fresh_is_old_enough(min: Age) -> Option<u64> {
        left(0, min)
    }

    /// The wait left is never zero: zero would mean old enough.
    #[should_fail]
    #[spec(fn(Age) -> Option<u64{v: v == 0}>)]
    fn nothing_left_to_wait(min: Age) -> Option<u64> {
        left(0, min)
    }

    /// A change in the future has waited nothing.
    #[should_fail]
    #[spec(fn() -> u64{v: v > 0})]
    fn future_has_waited() -> u64 {
        waited(5, 9)
    }
}
