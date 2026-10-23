//! What the running system says, each through its own command: the files
//! processes hold open (`lsof`), the programs running (`ps`), whether git
//! ignores a path (`git`), the apps installed (`plutil`, `mdfind`) and the
//! free space (`df`). The only place these command lines are built.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use diskreap_judge::Git;

fn run(program: &str, args: &[&str]) -> Result<(bool, String)> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| anyhow!("{program} cannot be run ({e}): it comes with macOS; check PATH"))?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Every path a process of the user holds open or works in, sorted; read
/// again when it is older than [`OPEN_FRESH`].
pub struct Open {
    paths: Vec<String>,
    read_at: Instant,
}

/// How long a reading of the open files is trusted while a run deletes.
pub const OPEN_FRESH: Duration = Duration::from_secs(30);

impl Open {
    pub fn read() -> Result<Open> {
        // `-F n` prints each open file's name; `cwd` and `txt` are among them.
        let (_, out) = run("lsof", &["-n", "-P", "-w", "-F", "n"])?;
        let mut paths: Vec<String> = out
            .lines()
            .filter_map(|l| l.strip_prefix('n'))
            .filter(|p| p.starts_with('/'))
            .map(|p| p.split(" (").next().unwrap_or(p).to_string())
            .collect();
        if paths.is_empty() {
            bail!(
                "lsof named no open file, so what is in use cannot be told: check that `lsof -n -F n` runs"
            );
        }
        paths.sort();
        paths.dedup();
        Ok(Open { paths, read_at: Instant::now() })
    }

    pub fn fresh(&mut self) -> Result<()> {
        if self.read_at.elapsed() > OPEN_FRESH {
            *self = Open::read()?;
        }
        Ok(())
    }

    /// Whether a process holds open, or works in, `path` or anything under it.
    pub fn holds(&self, path: &str) -> bool {
        if self.paths.binary_search_by(|p| p.as_str().cmp(path)).is_ok() {
            return true;
        }
        let under = format!("{path}/");
        let i = self.paths.partition_point(|p| p.as_str() < under.as_str());
        self.paths.get(i).is_some_and(|p| p.starts_with(&under))
    }
}

/// The programs running, by their executables' paths.
pub fn running() -> Result<Vec<String>> {
    let (ok, out) = run("ps", &["-axo", "comm="])?;
    if !ok {
        bail!("ps did not list the programs running: check that `ps -axo comm=` runs");
    }
    Ok(out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

/// Whether a program by this name runs: an executable of that name, or
/// anything inside `<name>.app`.
pub fn is_running(programs: &[String], name: &str) -> bool {
    let app = format!("/{name}.app/");
    programs.iter().any(|p| p.rsplit('/').next() == Some(name) || p.contains(&app))
}

/// Whether git ignores a path in a work tree around it.
pub fn git(path: &Path) -> Git {
    let Some(root) = path.parent().and_then(repo_root) else { return Git::Outside };
    let Ok(rel) = path.strip_prefix(&root) else { return Git::NotIgnored };
    let rel = rel.to_string_lossy();
    let root = root.to_string_lossy();
    let ignored = Command::new("git")
        .args(["-C", &root, "check-ignore", "-q", "--", &rel])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ignored {
        return Git::NotIgnored;
    }
    // A file added by force is tracked inside an ignored directory.
    match run("git", &["-C", &root, "ls-files", "--", &rel]) {
        Ok((true, out)) if out.trim().is_empty() => Git::Ignored,
        _ => Git::NotIgnored,
    }
}

fn repo_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

/// Where apps are installed.
fn app_dirs(home: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
        "/Library/Input Methods",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    dirs.push(Path::new(home).join("Applications"));
    dirs.push(Path::new(home).join("Library/Input Methods"));
    // An app may sit in a folder of its own in /Applications.
    let nested: Vec<PathBuf> = std::fs::read_dir("/Applications")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.extension().is_none_or(|e| e != "app"))
        .collect();
    dirs.extend(nested);
    dirs
}

/// The apps installed: their names and bundle ids, lowercase.
pub struct Apps {
    names: HashSet<String>,
    ids: HashSet<String>,
}

impl Apps {
    pub fn read(home: &str) -> Apps {
        let mut names = HashSet::new();
        let mut ids = HashSet::new();
        for dir in app_dirs(home) {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "app") {
                    continue;
                }
                if let Some(stem) = path.file_stem() {
                    names.insert(stem.to_string_lossy().to_lowercase());
                }
                let plist = path.join("Contents/Info.plist");
                let plist = plist.to_string_lossy();
                if let Ok((true, id)) =
                    run("plutil", &["-extract", "CFBundleIdentifier", "raw", "-o", "-", &plist])
                {
                    ids.insert(id.trim().to_lowercase());
                }
            }
        }
        Apps { names, ids }
    }

    /// Whether an app by this name or bundle id is installed; Spotlight is
    /// asked as well, for an app outside the usual folders.
    pub fn installed(&self, name: &str) -> bool {
        let n = name.to_lowercase();
        if self.names.contains(&n) || self.ids.contains(&n) {
            return true;
        }
        let query = format!("kMDItemCFBundleIdentifier == \"{}\"", name.replace('"', ""));
        matches!(run("mdfind", &[&query]), Ok((_, out)) if !out.trim().is_empty())
    }

    /// Whether any installed app's name or bundle id has `name` in it, or is in it.
    pub fn resembles(&self, name: &str) -> bool {
        let n = name.to_lowercase();
        self.names
            .iter()
            .chain(&self.ids)
            .any(|a| a.contains(&n) || (a.len() > 3 && n.contains(a.as_str())))
    }
}

/// The bytes free on the volume of `path`.
pub fn free(path: &str) -> Result<u64> {
    let (_, out) = run("df", &["-k", "-P", path])?;
    out.lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|k| k.parse::<u64>().ok())
        .map(|k| k.saturating_mul(1024))
        .ok_or_else(|| {
            anyhow!("df did not say the free space of {path}: check that `df -k -P {path}` runs")
        })
}
