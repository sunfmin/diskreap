//! The Rule list and what a Rule may reach: parsing the list kept in the gist,
//! checking each Rule, writing the list back, matching a Rule's glob against a
//! path, and the floor — the paths no Rule may delete however it is written
//! (docs/adr/0001-safety-from-confirmed-rules-and-a-fixed-floor.md). Nothing
//! here reads or writes: the effect crate hands in the places it read from the
//! environment and gets answers back.

extern crate flux_alloc;
extern crate flux_core;

use flux_rs::attrs::*;
use serde_json::{Map, Value};

/// The file the Rule list is kept in, inside the gist.
pub const FILE: &str = "diskreap-rules.json";
/// The description the gist is created with.
pub const GIST_DESCRIPTION: &str = "diskreap rules";

/// The longest Minimum age a Rule may set: ten years, in seconds.
pub const MAX_AGE: u64 = 315_360_000;

/// How long nothing in a Match may have changed before it may go, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[refined_by(secs: int)]
#[invariant(0 < secs && secs <= MAX_AGE)]
pub struct Age {
    #[field(u64[secs])]
    secs: u64,
}

impl Age {
    #[spec(fn(secs: u64{0 < secs && secs <= MAX_AGE}) -> Age[secs])]
    const fn new(secs: u64) -> Age {
        Age { secs }
    }

    /// The age in seconds, never zero.
    #[spec(fn(&Age[@s]) -> u64[s])]
    pub fn secs(&self) -> u64 {
        self.secs
    }
}

/// One day: what a Rule waits for unless it says otherwise.
pub const ONE_DAY: Age = Age::new(86_400);

/// Reads an age written as a whole number and a unit: `30m`, `12h`, `1d`, `2w`.
pub fn parse_age(text: &str) -> Result<Age, String> {
    let fault = || {
        format!(
            "the age `{text}` is not a whole number followed by m, h, d or w (minutes, hours, days, weeks): write it like `1d` or `12h`"
        )
    };
    let unit = text.chars().last().ok_or_else(fault)?;
    let per: u64 = match unit {
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        'w' => 604_800,
        _ => return Err(fault()),
    };
    let digits = text.strip_suffix(unit).ok_or_else(fault)?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(fault());
    }
    let count: u64 = digits.parse().map_err(|_| fault())?;
    let secs = count.checked_mul(per).ok_or_else(fault)?;
    if secs == 0 {
        return Err(format!("the age `{text}` is zero: a Rule waits at least a minute, `1m`"));
    }
    if secs > MAX_AGE {
        return Err(format!("the age `{text}` is past ten years: write `520w` or less"));
    }
    Ok(Age::new(secs))
}

/// Writes an age back in the largest unit that holds it whole.
pub fn age_text(age: Age) -> String {
    let secs = age.secs();
    if secs.is_multiple_of(604_800) {
        format!("{}w", secs / 604_800)
    } else if secs.is_multiple_of(86_400) {
        format!("{}d", secs / 86_400)
    } else if secs.is_multiple_of(3_600) {
        format!("{}h", secs / 3_600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// One kind of thing that may be deleted, as a person agreed to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub glob: String,
    /// The files of which one must sit beside a Match for it to count; none
    /// when the Rule needs no marker.
    pub marker: Vec<String>,
    pub min_age: Age,
    /// Names of programs: while any runs, nothing this Rule matches goes.
    pub unless_running: Vec<String>,
    /// App names or bundle ids: while any is installed, nothing this Rule matches goes.
    pub unless_installed: Vec<String>,
    pub why: String,
}

/// The keys a Rule is written with, every one it may have.
pub const KEYS: [&str; 7] =
    ["id", "glob", "marker", "min_age", "unless_running", "unless_installed", "why"];

/// The places a glob's variables stand for, as absolute paths with no trailing
/// `/`: the home directory, the temporary directory, and the user's own entry
/// under `/var/folders` (the parent of the temporary directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    pub home: String,
    pub tmpdir: String,
    pub var_folder: String,
}

impl Places {
    /// The places for a home and a temporary directory as the environment
    /// gives them. The user's `/var/folders` entry is the temporary
    /// directory's parent when it is macOS's `…/T`, and otherwise the
    /// temporary directory itself.
    pub fn new(home: &str, tmpdir: &str) -> Places {
        let home = trimmed(home);
        let tmpdir = trimmed(tmpdir);
        let var_folder = match tmpdir.rsplit_once('/') {
            Some((parent, "T")) if !parent.is_empty() => parent.to_string(),
            _ => tmpdir.clone(),
        };
        Places { home, tmpdir, var_folder }
    }
}

fn trimmed(path: &str) -> String {
    let t = path.trim_end_matches('/');
    if t.is_empty() { "/".to_string() } else { t.to_string() }
}

/// A Rule's id: lowercase letters, digits and `-`, starting with a letter.
pub fn id_fault(id: &str) -> Option<String> {
    let good = id.len() <= 64
        && id.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if good {
        None
    } else {
        Some(format!(
            "the id `{id}` is not a short name of lowercase letters, digits and `-` starting with a letter: write it like `cargo-target`"
        ))
    }
}

/// Reads one Rule from its JSON, naming every fault at once.
pub fn parse_rule(value: &Value, places: &Places) -> Result<Rule, Vec<String>> {
    let mut faults = Vec::new();
    let Some(object) = value.as_object() else {
        return Err(vec![format!(
            "a Rule is a JSON object with the keys {}: got `{value}`",
            KEYS.join(", ")
        )]);
    };
    for key in object.keys() {
        if !KEYS.contains(&key.as_str()) {
            faults.push(format!("a Rule has no key `{key}`: its keys are {}", KEYS.join(", ")));
        }
    }
    let id = text(object, "id", &mut faults).unwrap_or_default();
    if let Some(fault) = id_fault(&id) {
        faults.push(fault);
    }
    let glob = text(object, "glob", &mut faults).unwrap_or_default();
    let why = text(object, "why", &mut faults);
    if why.as_deref().is_some_and(|w| w.trim().is_empty()) {
        faults.push(format!(
            "the Rule `{id}` says no `why`: say why what it matches is safe to delete"
        ));
    }
    let why = why.unwrap_or_default();
    let marker = match object.get("marker") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(m)) => vec![m.clone()],
        Some(Value::Array(items)) => {
            items.iter().map(|m| m.as_str().unwrap_or_default().to_string()).collect()
        }
        Some(_) => vec![String::new()],
    };
    let file_name = |m: &String| !m.is_empty() && !m.contains('/');
    if !marker.iter().all(file_name) || object.get("marker") == Some(&Value::Array(Vec::new())) {
        faults.push(format!(
            "the Rule `{id}` has the marker `{}`: a marker is a file name with no `/`, or a list of them of which one must be there",
            object.get("marker").cloned().unwrap_or_default()
        ));
    }
    let min_age = match object.get("min_age") {
        None | Some(Value::Null) => ONE_DAY,
        Some(Value::String(t)) => parse_age(t).unwrap_or_else(|f| {
            faults.push(format!("the Rule `{id}`: {f}"));
            ONE_DAY
        }),
        Some(other) => {
            faults.push(format!(
                "the Rule `{id}` has the min_age `{other}`: write it as text like `1d`"
            ));
            ONE_DAY
        }
    };
    let unless_running = names(object, "unless_running", &id, &mut faults);
    let unless_installed = names(object, "unless_installed", &id, &mut faults);
    match compile(&glob, places) {
        Ok(compiled) => {
            if compiled.any_depth() && marker.is_empty() {
                faults.push(format!(
                    "the Rule `{id}` searches any depth (`**`) and names no marker: name the file that sits beside every Match, like `\"marker\": \"Cargo.toml\"`"
                ));
            }
        }
        Err(f) => faults.push(format!("the Rule `{id}`: {f}")),
    }
    if faults.is_empty() {
        Ok(Rule { id, glob, marker, min_age, unless_running, unless_installed, why })
    } else {
        Err(faults)
    }
}

fn text(object: &Map<String, Value>, key: &str, faults: &mut Vec<String>) -> Option<String> {
    match object.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => {
            faults.push(format!("a Rule's `{key}` is text: got `{other}`"));
            None
        }
        None => {
            faults.push(format!("a Rule needs `{key}`"));
            None
        }
    }
}

fn names(
    object: &Map<String, Value>,
    key: &str,
    id: &str,
    faults: &mut Vec<String>,
) -> Vec<String> {
    match object.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                match item.as_str() {
                    Some(s) if !s.trim().is_empty() => out.push(s.to_string()),
                    _ => faults.push(format!(
                        "the Rule `{id}` has `{item}` in `{key}`: each is a program's or an app's name, as text"
                    )),
                }
            }
            out
        }
        Some(other) => {
            faults.push(format!("the Rule `{id}`'s `{key}` is a list of names: got `{other}`"));
            Vec::new()
        }
    }
}

/// Reads the whole Rule list, naming every fault of every Rule at once.
pub fn parse_list(text: &str, places: &Places) -> Result<Vec<Rule>, Vec<String>> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        vec![format!("the Rule list in the gist's {FILE} is not JSON ({e}): fix it in the gist")]
    })?;
    let Some(items) = value.get("rules").and_then(Value::as_array) else {
        return Err(vec![format!(
            "the gist's {FILE} is not an object with a `rules` list: fix it in the gist to `{{\"rules\": []}}` and the Rules"
        )]);
    };
    let mut rules: Vec<Rule> = Vec::new();
    let mut faults = Vec::new();
    for item in items {
        match parse_rule(item, places) {
            Ok(rule) => {
                if rules.iter().any(|r| r.id == rule.id) {
                    faults.push(format!(
                        "two Rules have the id `{}`: rename one in the gist",
                        rule.id
                    ));
                }
                rules.push(rule);
            }
            Err(mut f) => faults.append(&mut f),
        }
    }
    if faults.is_empty() { Ok(rules) } else { Err(faults) }
}

/// One Rule as it is written into the list.
pub fn rule_json(rule: &Rule) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), Value::String(rule.id.clone()));
    object.insert("glob".into(), Value::String(rule.glob.clone()));
    match (rule.marker.first(), rule.marker.len()) {
        (None, _) => {}
        (Some(one), 1) => {
            object.insert("marker".into(), Value::String(one.clone()));
        }
        (Some(_), _) => {
            object.insert("marker".into(), strings(&rule.marker));
        }
    }
    object.insert("min_age".into(), Value::String(age_text(rule.min_age)));
    if !rule.unless_running.is_empty() {
        object.insert("unless_running".into(), strings(&rule.unless_running));
    }
    if !rule.unless_installed.is_empty() {
        object.insert("unless_installed".into(), strings(&rule.unless_installed));
    }
    object.insert("why".into(), Value::String(rule.why.clone()));
    Value::Object(object)
}

fn strings(names: &[String]) -> Value {
    Value::Array(names.iter().cloned().map(Value::String).collect())
}

/// The whole list as it is written into the gist: Rules in the order given.
pub fn list_text(rules: &[Rule]) -> String {
    let mut list = Map::new();
    list.insert("rules".into(), Value::Array(rules.iter().map(rule_json).collect()));
    let value = Value::Object(list);
    let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
    text.push('\n');
    text
}

/// One `/`-separated part of a glob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// `**`: any number of directories, none included.
    AnyDepth,
    /// A name, where `*` stands for any run of characters and `?` for one.
    Name(Vec<Piece>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Char(char),
    One,
    Many,
}

/// A glob with its variable expanded: the directory it starts from, which
/// holds no wildcard, and the parts after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    pub base: String,
    pub segments: Vec<Segment>,
}

impl Glob {
    /// Whether it searches any depth.
    pub fn any_depth(&self) -> bool {
        self.segments.contains(&Segment::AnyDepth)
    }
}

/// Expands a glob's variable and splits it, refusing what the floor forbids:
/// a glob starts at `~/`, `$TMPDIR/` or `$VARFOLDER/`, holds no `.` or `..`,
/// does not end in `**`, and cannot reach the shallow directories the floor
/// keeps (see [`floor`]).
pub fn compile(glob: &str, places: &Places) -> Result<Glob, String> {
    let (root, rest) = if let Some(rest) = glob.strip_prefix("~/") {
        (places.home.as_str(), rest)
    } else if let Some(rest) = glob.strip_prefix("$TMPDIR/") {
        (places.tmpdir.as_str(), rest)
    } else if let Some(rest) = glob.strip_prefix("$VARFOLDER/") {
        (places.var_folder.as_str(), rest)
    } else {
        return Err(format!(
            "the glob `{glob}` does not start at `~/`, `$TMPDIR/` or `$VARFOLDER/`: start it at one of them"
        ));
    };
    let mut base = root.to_string();
    let mut segments = Vec::new();
    for part in rest.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!(
                "the glob `{glob}` has an empty, `.` or `..` part: write each directory by name"
            ));
        }
        if part == "**" {
            segments.push(Segment::AnyDepth);
        } else if part.contains("**") {
            return Err(format!(
                "the glob `{glob}` has `**` inside a name: `**` is a part of its own"
            ));
        } else if segments.is_empty() && !part.contains(['*', '?']) {
            base = format!("{base}/{part}");
        } else {
            segments.push(Segment::Name(pieces(part)));
        }
    }
    if segments.last().is_none_or(|s| *s == Segment::AnyDepth) {
        if segments.is_empty() {
            // A glob of plain names matches one path: the floor decides it.
            if let Some(why) = floor(&base, places) {
                return Err(format!("the glob `{glob}` reaches {base}, which {why}"));
            }
            return Ok(Glob { base, segments });
        }
        return Err(format!(
            "the glob `{glob}` ends in `**`: end it with the name of what it matches"
        ));
    }
    // The shallowest path it can match: the base and one name per part,
    // `**` standing for none.
    let mut shallowest = base.clone();
    for segment in &segments {
        if let Segment::Name(_) = segment {
            shallowest.push_str("/x");
        }
    }
    // With `**` a Match can be deeper than that, and the floor keeps the
    // shallow ones when they are found.
    let any_depth = segments.contains(&Segment::AnyDepth);
    if let (false, Some(why)) = (any_depth, floor(&shallowest, places)) {
        return Err(format!(
            "the glob `{glob}` can match a path as shallow as {shallowest}, which {why}"
        ));
    }
    Ok(Glob { base, segments })
}

fn pieces(name: &str) -> Vec<Piece> {
    name.chars()
        .map(|c| match c {
            '*' => Piece::Many,
            '?' => Piece::One,
            other => Piece::Char(other),
        })
        .collect()
}

/// Whether a name matches a glob part's pieces.
pub fn name_matches(pieces: &[Piece], name: &[char]) -> bool {
    let Some((piece, rest)) = pieces.split_first() else {
        return name.is_empty();
    };
    let next = name.split_first();
    match (piece, next) {
        (Piece::Many, _) => {
            name_matches(rest, name) || next.is_some_and(|(_, tail)| name_matches(pieces, tail))
        }
        (Piece::One, Some((_, tail))) => name_matches(rest, tail),
        (Piece::Char(c), Some((first, tail))) if first == c => name_matches(rest, tail),
        _ => false,
    }
}

/// Whether the names of a path below a glob's base match its parts.
pub fn segments_match(segments: &[Segment], names: &[Vec<char>]) -> bool {
    let Some((segment, rest)) = segments.split_first() else {
        return names.is_empty();
    };
    let next = names.split_first();
    match (segment, next) {
        (Segment::AnyDepth, _) => {
            segments_match(rest, names)
                || next.is_some_and(|(_, tail)| segments_match(segments, tail))
        }
        (Segment::Name(pieces), Some((first, tail))) => {
            name_matches(pieces, first) && segments_match(rest, tail)
        }
        (Segment::Name(_), None) => false,
    }
}

/// Whether a path matches a glob: it is the base, or below it with names that
/// match the glob's parts.
pub fn path_matches(glob: &Glob, path: &str) -> bool {
    match below(&glob.base, path) {
        Some(rest) => {
            let names: Vec<Vec<char>> = if rest.is_empty() {
                Vec::new()
            } else {
                rest.split('/').map(|n| n.chars().collect()).collect()
            };
            segments_match(&glob.segments, &names)
        }
        None => false,
    }
}

/// The part of `path` under `dir`, with no leading `/`; empty for `dir`
/// itself, `None` when `path` is not `dir` or below it.
pub fn below<'a>(dir: &str, path: &'a str) -> Option<&'a str> {
    let rest = path.strip_prefix(dir)?;
    if rest.is_empty() || dir == "/" { Some(rest) } else { rest.strip_prefix('/') }
}

/// How many names a path has below a directory.
fn depth(rest: &str) -> usize {
    if rest.is_empty() { 0 } else { rest.split('/').count() }
}

/// Why the floor keeps a path, whatever Rule matched it; `None` when the floor
/// lets it be judged. The floor keeps every path outside the home directory
/// and the user's own `/var/folders` entry, those two themselves and every
/// directory directly in them, every directory directly in `~/Library`, and
/// any path written with a `.` or `..` part.
pub fn floor(path: &str, places: &Places) -> Option<&'static str> {
    if path.split('/').any(|p| p == "." || p == "..") {
        return Some("is written with `.` or `..`");
    }
    if let Some(rest) = below(&places.var_folder, path) {
        return if depth(rest) < 2 {
            Some("is the user's /var/folders entry or a directory directly in it")
        } else {
            None
        };
    }
    let Some(rest) = below(&places.home, path) else {
        return Some("is outside the home directory and the user's /var/folders entry");
    };
    if depth(rest) < 2 {
        return Some("is the home directory or a directory directly in it");
    }
    if let Some(library) = below("Library", rest)
        && depth(library) < 2
    {
        return Some("is ~/Library or a directory directly in it");
    }
    None
}

/// The directories a `**` does not go into unless the glob's base is already
/// inside them: `~/Library`, and every hidden directory (its name starts with
/// `.`), where tools keep what they installed: a `node_modules` in
/// `~/.pi/...` is a program, not a project's build. A hidden directory can
/// still be a Match itself (`.venv`); only what is inside it is not looked at.
/// A `.git` directory is never gone into.
pub fn skipped(dir: &str, name: &str, glob: &Glob, places: &Places) -> bool {
    if name == ".git" {
        return true;
    }
    let hidden = name.starts_with('.');
    if !hidden && (dir != places.home || name != "Library") {
        return false;
    }
    let child = format!("{dir}/{name}");
    below(&child, &glob.base).is_none()
}

/// Whether a walk for a glob reaches `path` as a Match: the path matches it,
/// and no directory between the glob's base and the path is one the walk does
/// not go into.
pub fn reached(glob: &Glob, path: &str, places: &Places) -> bool {
    if !path_matches(glob, path) {
        return false;
    }
    let Some(rest) = below(&glob.base, path) else { return false };
    let names: Vec<&str> = rest.split('/').collect();
    let Some((_, between)) = names.split_last() else { return true };
    let mut dir = glob.base.clone();
    for name in between {
        if skipped(&dir, name, glob, places) {
            return false;
        }
        dir = format!("{dir}/{name}");
    }
    true
}

#[cfg(test)]
mod tests;

/// What the refinements forbid, each written the way a careless caller would:
/// Flux must refuse every one.
#[cfg(flux)]
#[allow(dead_code)]
mod misuse {
    use super::*;

    /// An age of nothing would let a Rule delete what was written a moment ago.
    #[should_fail]
    fn zero_age() -> Age {
        Age::new(0)
    }

    /// Past ten years is a typo, not a Minimum age.
    #[should_fail]
    fn age_past_ten_years() -> Age {
        Age::new(MAX_AGE + 1)
    }

    /// An Age's seconds are never zero.
    #[should_fail]
    #[spec(fn(Age) -> u64[0])]
    fn age_of_nothing(age: Age) -> u64 {
        age.secs()
    }
}
