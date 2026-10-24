//! The command line: what each command takes, and the manual written beside
//! it. `--help` answers the whole manual, `<command> --help` (or `-h`) one
//! command's part, `help <topic>` a topic; `-h` alone is one screen.

use std::ffi::OsString;

use agent_help::{Help, Topic};
use clap::{CommandFactory, Parser, Subcommand};
use diskreap_rules::{AgeUnit, FILE, ONE_DAY, Root, age_text, listed};

use crate::run::Event;
use crate::{VERSION, gh, scan, system, walk};

const ANYONE: &str = "Run by anyone: these change nothing";
const AGREED: &str = "Run by a person, or an agent with that person's yes: these change the gist";
const DELETES: &str = "Run by a person, or an agent a person asked to free space: this deletes";

/// diskreap frees disk space on this Mac by Rules kept in a private gist, and deletes only what is safe.
///
/// A Rule names one kind of thing that may be deleted: a file glob, the conditions under which a Match may go, and why it is safe. Every Rule exists because a person agreed to it. The Rule list lives in one private gist of yours (file `{const:FILE}`) and is read again on every run; diskreap keeps nothing on this machine.
///
/// **What it needs**: macOS, and `gh` logged in with the gist scope (`gh auth login`); `lsof`, `ps`, `git`, `plutil`, `mdfind` and `df`, which come with macOS and Xcode's tools.
///
/// **Run first**:
///
/// - `diskreap doctor`: what is missing, each with its fix.
/// - `diskreap init`: makes the gist when there is none.
///
/// The gist's id is remembered in `{const:CACHE}` so a run need not list every gist; it is only a shortcut, looked for again when it is gone.
///
/// **What a run does**: each Rule's glob is walked. A Match goes only when every check passes:
///
/// - The floor: never `/`, `~`, `~/Library` or anything directly in either; never outside `~` and your own `/var/folders` entry; never a git repository or anything git tracks; never through a symlink or onto another volume; never another user's file. No Rule can change the floor.
/// - Nothing anywhere in it was changed or used within the Rule's `min_age` (default `{const:MIN_AGE}`): a file's last use is the newest of its writes and its access time, a read counting as `{const:READ_LAG}` later than the access time says (macOS moves it at most daily); where no access time can be read, the writes alone.
/// - No process holds a file in it open or works in it (`lsof`).
/// - None of the Rule's `unless_running` programs runs, and none of its `unless_installed` apps is installed.
/// - With a `marker`, that file (or one of the list) sits beside it (`Cargo.toml` beside `target`, a lockfile beside `node_modules`).
///
/// Anything that fails a check is held, and the answer says why.
///
/// **The way to clean**:
///
/// 1. `diskreap plan --all` lists every Match and its verdict, as it walks.
/// 2. Read every path it would delete; drop any that is not a project's build or a cache (see `diskreap clean --help`), and say which and why.
/// 3. `diskreap clean --path <path>` for each one left, or the paths a line each on stdin with `--path -`. Each is judged again as it goes.
///
/// **The way to add a Rule**:
///
/// 1. `diskreap scan`, or look yourself, to find what takes space.
/// 2. Write the Rule (see [how](help:writing-a-rule), and [the Rules one person uses](help:rule-examples)); `diskreap rule check '<rule>'` shows what it would match.
/// 3. Show the person the Rule and that answer; add it only when they agree: `diskreap rule add '<rule>'`.
///
/// **Output**: every command prints one JSON object on stdout when it ends; a failure is `{"error": "..."}` with exit 1, and says the line that fixes it. The exceptions: `--help`, `-h`, `help` and `--version` print text; and while `plan`, `clean`, `rule check`, `rule add` and `scan` walk, each Match, deletion and finding is printed as it happens, one JSON object a line on stderr, so a long run can be watched, and acted on, as it goes. Its `event` says what happened:
///
/// {table:event}
///
/// Walks run on several threads; Rules that search any depth from the same place share one walk, so the events of several Rules come interleaved. Byte counts are allocated blocks: an APFS clone counts in each copy, so they can add up to more than `clean` frees. `clean` measures what it freed.
#[derive(Parser)]
#[command(
    name = "diskreap",
    version = VERSION,
    verbatim_doc_comment,
    disable_help_subcommand = true,
    after_help = "One JSON object on stdout; events, one a line, on stderr as it walks.\nRead `diskreap --help` before the first run."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Check everything diskreap needs, changing nothing.
    ///
    /// **When**: first, and whenever another command fails for a reason outside it. **Reads** gh's login, the gist and its Rule list, `lsof`, `git`, `df`.
    ///
    /// **Answers**:
    ///
    /// - `ok`: true when every check passed.
    /// - `findings`: each check: `check`, `ok`, `says`, and `fix` when it failed.
    /// - `places`: what {const:ROOT_NAMES} stand for on this machine.
    ///
    /// **Trips you up**: it always exits 0: read `ok`.
    #[command(verbatim_doc_comment, next_help_heading = ANYONE)]
    Doctor,
    /// Make the private gist that keeps the Rule list, if there is none.
    ///
    /// **When**: once, before anything else. Running it again changes nothing. **Reads** your gists, through `gh`. **Writes** a new private gist holding `{const:FILE}` with no Rules, when none of your gists holds it, and its id to `{const:CACHE}`.
    ///
    /// **Answers**:
    ///
    /// - `gist`: the gist's id.
    /// - `created`: true when this run made it.
    /// - `rules`: how many Rules it holds.
    /// - `file`: `{const:FILE}`.
    /// - `cache`: where the gist's id is remembered.
    ///
    /// **Trips you up**: two gists holding `{const:FILE}` is an error: delete one.
    #[command(verbatim_doc_comment, next_help_heading = AGREED)]
    Init,
    /// Say what clean would delete and what it would hold, changing nothing.
    ///
    /// `diskreap plan [--rule ID] [--path P] [--all]`
    ///
    /// **When**: before `clean`, to see what it would do. **Reads** the Rule list from the gist, every Match of each Rule and everything in it, `lsof`, `ps`, `git` and the apps installed. **Writes** nothing.
    ///
    /// - `--rule ID`: only this Rule.
    /// - `--path P`: only `P`, when it is a Match of a Rule: its glob reaches it and its marker is beside it. Give it once for each path, or `--path -` for paths a line each on stdin. Only these are read, so it is quick. No symlink, `.` or `..` may lead to `P`.
    /// - `--all`: every Match, not only the {const:LARGEST} largest of each Rule.
    ///
    /// **Answers**:
    ///
    /// - `gist`: the gist's id.
    /// - `rules`: for each Rule id:
    ///   - `glob`: its glob.
    ///   - `matched`: how many Matches it has.
    ///   - `deletable`, `deletable_bytes`: those `clean` would delete, and their size.
    ///   - `held`, `held_bytes`: those it would keep, and their size.
    ///   - `holds`: how many are held for each reason: {const:HOLDS}.
    ///   - `largest`: the {const:LARGEST} largest Matches (`matches`, with `--all`): `path`, `bytes`, `verdict` (`delete` or `hold`), `holds` (why, in words), `bytes_at_least` (true when a held Match was not read to the end: reading stops at the first thing that holds it).
    ///   - `seconds`: how long its walk took; Rules that search any depth from the same place share one walk, and its time.
    ///   - `error`: instead of the above, when its glob no longer compiles.
    /// - `matched`, `deletable`, `deletable_bytes`: the totals.
    /// - `note`: what the byte counts mean.
    /// - `paths`: with `--path`, the paths judged.
    /// - `unmatched`: with `--path`, those no Rule matches.
    /// - `why`: with `--path`, when some match no Rule: why that can be.
    /// - `seconds`: how long the run took.
    ///
    /// **Events** (stderr): `start` for each Rule, then `match` for each Match, then `rule` when a Rule is done.
    ///
    /// **Trips you up**: a Match held as recent may go on a later run: `holds` say when.
    #[command(verbatim_doc_comment, next_help_heading = ANYONE)]
    Plan {
        #[arg(long)]
        rule: Option<String>,
        #[arg(long)]
        path: Vec<std::path::PathBuf>,
        #[arg(long)]
        all: bool,
    },
    /// Delete the paths named that are Matches nothing holds.
    ///
    /// `diskreap clean --path P... [--rule ID] [--all]`
    ///
    /// **When**: after `plan`, once every path it would delete was looked over. **Reads** as `plan` does, for the paths named only. **Writes**: deletes each path named that is a Match whose verdict is `delete`, the moment it is judged. A file or symlink is removed by itself; a directory with all it holds, never following a symlink inside it. The open files are read again every {const:OPEN_FRESH} seconds.
    ///
    /// - `--path P`: a path to delete, when it is a Match of a Rule: its glob reaches it and its marker is beside it. Give it once for each path, or `--path -` for paths a line each on stdin. Needed: `clean` deletes nothing it was not given. Only these are read, so it is quick. No symlink, `.` or `..` may lead to `P`.
    /// - `--rule ID`: only this Rule.
    /// - `--all`: every Match, not only the {const:LARGEST} largest of each Rule.
    ///
    /// **Answers** everything `plan` answers (`paths`, `unmatched`, `why` too), and:
    ///
    /// - `rules.<id>.deleted`, `deleted_bytes`: what was deleted for each Rule.
    /// - `rules.<id>.failed`: `[{path, error}]` that could not be.
    /// - `deleted`, `deleted_bytes`, `failed`: the totals.
    /// - `free_before`, `free_after`, `freed_bytes`: free space on the home volume, measured before and after.
    ///
    /// **Events** (stderr): as `plan`'s, with `deleted` or `failed` in place of `match` for a Match that went or could not.
    ///
    /// **Trips you up**: `freed_bytes` is what `df` measured; it is less than `deleted_bytes` when files were APFS clones, and anything else writing at the time counts too. Stopping it half way loses nothing: run it again and it finishes.
    ///
    /// **Look before it goes**: a Rule can match more than it meant to. Before `clean`, read every path `plan` would delete and drop any that is not a build or a cache a project makes: an installed program (`releases/`, `versions/`, `extensions/`, `lib/node_modules` of a tool, inside `site-packages` or an `.app`), anything in a hidden or `Library` directory, a person's own work. Tell the person what you dropped and why; if one Rule keeps matching such paths, propose fixing the Rule.
    #[command(verbatim_doc_comment, next_help_heading = DELETES)]
    Clean {
        #[arg(long)]
        rule: Option<String>,
        #[arg(long)]
        path: Vec<std::path::PathBuf>,
        #[arg(long)]
        all: bool,
    },
    /// Find large directories no Rule covers, deleting nothing.
    ///
    /// **When**: to find what to write the next Rule for. What it finds becomes a Rule only after a person agrees. **Reads** everything under `~` (not looking into what a Rule covers, nor other volumes), `$TMPDIR`, `~/Library/Application Support`, the apps installed and the programs running. **Writes** nothing. Walking `~` can take minutes.
    ///
    /// **Answers**:
    ///
    /// - `gist`: the gist's id.
    /// - `large`: up to {const:SHOWN} directories of {const:LARGE} or more with no child that large, largest first: `path`, `bytes`.
    /// - `tmpdir`: `$TMPDIR`'s entries grouped by what made them (`bk-catalog-10096-0` and `bk-catalog-11062-12` are both `bk-catalog`), largest first: `group`, `glob`, `entries`, `bytes`.
    /// - `orphans`: Application Support directories of {const:ORPHAN_MIN} or more that no installed app's name or bundle id resembles: `path`, `bytes`, `why`, and `rule`, a draft to complete and check.
    /// - `seconds`: how long it took.
    /// - `next`: what to do with the findings.
    ///
    /// **Events** (stderr): `start` (with `scan`) as each part starts, and `large`, `tmpdir` or `orphan` as each is found.
    ///
    /// **Trips you up**: it guesses: a large directory may be your work, and an orphan may belong to an app it does not know. Ask the person before any becomes a Rule.
    #[command(verbatim_doc_comment, next_help_heading = ANYONE)]
    Scan,
    /// Read and change the Rule list in the gist.
    ///
    /// Each of its commands is in this manual as `diskreap rule <command>`; [how a Rule is written](help:writing-a-rule) is a topic of its own.
    #[command(subcommand, verbatim_doc_comment, next_help_heading = ANYONE)]
    Rule(RuleCommand),
    /// Show the whole manual, or one topic of it.
    ///
    /// `diskreap help [TOPIC]`: with no topic, the whole manual, as `--help` shows it; with one, that topic alone, such as `diskreap help writing-a-rule`.
    #[command(verbatim_doc_comment, next_help_heading = ANYONE)]
    Help { topic: Option<String> },
}

#[derive(Subcommand)]
pub enum RuleCommand {
    /// Print the Rule list.
    ///
    /// **Reads** the gist. **Writes** nothing.
    ///
    /// **Answers**:
    ///
    /// - `gist`: the gist's id.
    /// - `rules`: each Rule, with its keys: {const:RULE_KEYS}; those it does not set left out.
    #[command(verbatim_doc_comment)]
    List,
    /// Say what a Rule would match and hold, without adding it.
    ///
    /// `diskreap rule check RULE [--all]`
    ///
    /// **When**: before `rule add`: show its answer to the person who decides. **Reads** the gist and what the Rule matches. **Writes** nothing.
    ///
    /// - `RULE`: the Rule as JSON ([how it is written](help:writing-a-rule)), or `-` to read it from stdin.
    /// - `--all`: every Match, not only the {const:LARGEST} largest.
    ///
    /// **Answers**:
    ///
    /// - `gist`: the gist's id.
    /// - `rule`: the Rule as it would be written.
    /// - `id_taken`: whether a Rule in the list has its id.
    /// - `same_as_listed`: whether that Rule is this one.
    /// - `plan`: what `plan` answers for one Rule (see `diskreap plan --help`).
    ///
    /// **Trips you up**: a Rule with a fault is an error naming every fault: a glob not starting at {const:ROOTS}, one that can only reach what the floor keeps, `**` with no marker, an unknown key, a bad `min_age`.
    #[command(verbatim_doc_comment)]
    Check {
        rule: String,
        #[arg(long)]
        all: bool,
    },
    /// Add a Rule a person agreed to.
    ///
    /// `diskreap rule add RULE`
    ///
    /// **When**: after `rule check`, once the person said yes. Never on your own. **Reads** the gist. **Writes** the Rule at the end of the Rule list, unless the gist changed since it was read (then it fails: run it again).
    ///
    /// - `RULE`: the Rule as JSON ([how it is written](help:writing-a-rule), [examples](help:rule-examples)), or `-` to read it from stdin.
    ///
    /// **Answers**:
    ///
    /// - `gist`: the gist's id.
    /// - `added`: true when it was added; false when the same Rule is there already.
    /// - `rule`: the Rule as written.
    /// - `plan`: what `plan` answers for it now.
    ///
    /// **Trips you up**: a different Rule with the same id is an error: `rule rm` it, or pick another id.
    #[command(verbatim_doc_comment, next_help_heading = AGREED)]
    Add { rule: String },
    /// Change a Rule's Minimum age or why.
    ///
    /// `diskreap rule set ID --min-age AGE | --why TEXT`
    ///
    /// **Reads** the gist. **Writes** the changed Rule, when something changed.
    ///
    /// - `--min-age AGE`: a whole number then {const:AGE_UNITS}, like `{const:MIN_AGE}`.
    /// - `--why TEXT`: why what it matches is safe to delete.
    ///
    /// **Answers**:
    ///
    /// - `gist`: the gist's id.
    /// - `changed`: false when the Rule already said this.
    /// - `rule`: the Rule as it is now.
    ///
    /// **Trips you up**: to change a glob, marker or unless list: `rule rm`, then `rule add`.
    #[command(verbatim_doc_comment, next_help_heading = AGREED)]
    Set {
        id: String,
        #[arg(long)]
        min_age: Option<String>,
        #[arg(long)]
        why: Option<String>,
    },
    /// Remove a Rule.
    ///
    /// `diskreap rule rm ID`
    ///
    /// **Reads** the gist. **Writes** the Rule list without it.
    ///
    /// **Answers**:
    ///
    /// - `gist`: the gist's id.
    /// - `removed`: false when no Rule had the id.
    /// - `id`: the id.
    /// - `rules`: how many Rules are left.
    #[command(verbatim_doc_comment, next_help_heading = AGREED)]
    Rm { id: String },
}

/// A Rule is one JSON object, such as:
///
/// ```jsonl
/// {"id": "cargo-target", "glob": "~/**/target", "marker": "Cargo.toml", "min_age": "1d", "why": "cargo builds it again"}
/// ```
///
/// Its keys:
///
/// {table:rule-key}
///
/// Check what it matches on this machine with `diskreap rule check '<rule>'` before a person agrees to it; [the Rules one person uses](help:rule-examples) show more.
#[derive(Topic)]
#[topic(name = "writing-a-rule")]
pub struct WritingARule;

/// Rules one person uses, one a line. Each is a pattern, not a default: check what it matches on this machine (`diskreap rule check`) and ask before adding it.
///
/// ```jsonl
/// {"id":"bk-tmp","glob":"$TMPDIR/bk-*","min_age":"1d","why":"bizkernel tests leave their working directories here; nothing reads them after the test ends"}
/// {"id":"go-build-tmp","glob":"$TMPDIR/go-build*","min_age":"1d","why":"go test and go build leave their work directories here"}
/// {"id":"chrome-code-sign-clone","glob":"$VARFOLDER/X/com.google.Chrome.code_sign_clone/*","min_age":"1d","unless_running":["Google Chrome"],"why":"Chrome clones its app bundle on each launch and makes a new one the next time it starts"}
/// {"id":"xcode-derived-data","glob":"~/Library/Developer/Xcode/DerivedData/*","min_age":"1d","why":"Xcode builds it again"}
/// {"id":"go-build-cache","glob":"~/Library/Caches/go-build","min_age":"1d","why":"the Go build cache; go fills it again"}
/// {"id":"homebrew-cache","glob":"~/Library/Caches/Homebrew/*","min_age":"1d","why":"downloads brew already installed; brew fetches them again if needed"}
/// {"id":"cargo-target","glob":"~/**/target","marker":"Cargo.toml","min_age":"1d","why":"cargo builds it again"}
/// {"id":"node-modules","glob":"~/**/node_modules","marker":["package-lock.json","pnpm-lock.yaml","yarn.lock","bun.lock","bun.lockb"],"min_age":"1d","why":"the package manager installs it again"}
/// {"id":"python-venv","glob":"~/**/.venv","marker":"pyproject.toml","min_age":"1d","why":"uv sync makes it again"}
/// {"id":"swiftpm-build","glob":"~/**/.build","marker":"Package.swift","min_age":"1d","why":"SwiftPM builds it again from Package.swift"}
/// {"id":"next-build","glob":"~/**/.next","marker":["next.config.js","next.config.mjs","next.config.ts"],"min_age":"1d","why":"next build / next dev builds it again"}
/// ```
#[derive(Topic)]
#[topic(name = "rule-examples")]
pub struct RuleExamples;

/// A key a Rule is written with: the help of each of
/// [`diskreap_rules::KEYS`], in its order.
#[derive(agent_help::Help)]
#[allow(dead_code)] // only its help is read
enum RuleKey {
    /// Needed. A short name: lowercase letters, digits and `-`, starting with a letter.
    Id,
    /// Needed. Starts at {const:ROOT_PLACES}; `*` and `?` in a name, `**` for any depth of directories, which does not go into `~/Library` or any hidden directory (`.name`: where tools keep what they installed) unless the glob starts inside it, nor ever into `.git`; a hidden directory can still be a Match itself (`.venv`).
    Glob,
    /// A file that must sit beside a Match, or a list of files of which one must; needed with `**`. Pick a file only a project has, not an installed copy: a lockfile beside `node_modules`, not `package.json`, which every installed package has too.
    Marker,
    /// How long nothing in a Match may have been changed or used: a whole number then {const:AGE_UNITS}; default `{const:MIN_AGE}`.
    MinAge,
    /// Programs: while one runs, nothing goes.
    UnlessRunning,
    /// App names or bundle ids: while one is installed, nothing goes.
    UnlessInstalled,
    /// Needed. Why what it matches is safe to delete.
    Why,
}

/// The manual, assembled from the command tree, its topics and tables, and
/// the values in code it quotes.
pub fn manual() -> Help {
    let bytes = |n: u64| match n {
        n if n >= 1 << 30 => format!("{} GiB", n >> 30),
        n => format!("{} MiB", n >> 20),
    };
    let names =
        |names: &[&str]| names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ");
    let roots = |text: fn(Root) -> String| Root::ALL.map(text);
    let units = AgeUnit::ALL.map(|u| format!("`{}` ({})", u.letter(), u.name()));
    Help::new(Cli::command())
        .topic::<WritingARule>()
        .topic::<RuleExamples>()
        .table::<Event>("event")
        .table::<RuleKey>("rule-key")
        .constant("FILE", FILE)
        .constant("CACHE", format!("~/{}", gh::CACHE))
        .constant("MIN_AGE", age_text(ONE_DAY))
        .constant("READ_LAG", format!("{}d", walk::READ_LAG / 86_400))
        .constant("HOLDS", names(&diskreap_judge::HOLD_KEYS))
        .constant("RULE_KEYS", names(&diskreap_rules::KEYS))
        .constant("ROOTS", listed(&roots(|r| format!("`{}/`", r.name())), "or"))
        .constant(
            "ROOT_PLACES",
            listed(&roots(|r| format!("`{}/` ({})", r.name(), r.what())), "or"),
        )
        .constant("ROOT_NAMES", listed(&roots(|r| format!("`{}`", r.name())), "and"))
        .constant("AGE_UNITS", listed(&units, "or"))
        .constant("LARGEST", diskreap_judge::LARGEST)
        .constant("OPEN_FRESH", system::OPEN_FRESH.as_secs())
        .constant("SHOWN", scan::SHOWN)
        .constant("LARGE", bytes(scan::LARGE))
        .constant("ORPHAN_MIN", bytes(scan::ORPHAN_MIN))
}

/// The one screen `-h` answers when no command is named.
pub fn screen(args: &[OsString]) -> Option<String> {
    let words: Vec<_> = args.iter().skip(1).map(|a| a.to_string_lossy()).collect();
    let short = words.iter().any(|w| w == "-h") && words.iter().all(|w| w.starts_with('-'));
    short.then(|| Cli::command().render_help().to_string())
}

#[cfg(test)]
mod tests {
    use agent_help::Table;

    #[test]
    fn every_rule_key_has_its_help() {
        let named: Vec<&str> = super::RuleKey::ENTRIES.iter().map(|e| e.name).collect();
        assert_eq!(named, diskreap_rules::KEYS);
    }
}
