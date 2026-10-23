//! Finding a Rule's Matches on disk, and reading what is true of each one's
//! tree. Nothing follows a symlink, and nothing goes onto another volume.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use diskreap_rules::{Glob, Places, Segment, name_matches, skipped};

/// Hands each path the glob matches that has the marker beside it to `found`
/// as soon as it is found, not looking inside a Match for more.
pub fn matches(
    glob: &Glob,
    marker: Option<&str>,
    places: &Places,
    dev: u64,
    found: &mut dyn FnMut(PathBuf),
) {
    let base = Path::new(&glob.base);
    // A base reached through a symlink is not where it says it is.
    match std::fs::canonicalize(base) {
        Ok(real) if real == base => {}
        _ => return,
    }
    let mut seen = HashSet::new();
    expand(base, &glob.segments, glob, marker, places, dev, found, &mut seen);
}

#[allow(clippy::too_many_arguments)]
fn expand(
    dir: &Path,
    segments: &[Segment],
    glob: &Glob,
    marker: Option<&str>,
    places: &Places,
    dev: u64,
    found: &mut dyn FnMut(PathBuf),
    seen: &mut HashSet<PathBuf>,
) {
    let Some((segment, rest)) = segments.split_first() else {
        if std::fs::symlink_metadata(dir).is_ok()
            && marker.is_none_or(|m| dir.parent().is_some_and(|p| p.join(m).exists()))
            && seen.insert(dir.to_path_buf())
        {
            found(dir.to_path_buf());
        }
        return;
    };
    let dir_text = dir.to_string_lossy();
    let children = children(dir, dev);
    match segment {
        Segment::AnyDepth => {
            expand(dir, rest, glob, marker, places, dev, found, seen);
            for (name, path, is_dir) in children {
                if is_dir && !seen.contains(&path) && !skipped(&dir_text, &name, glob, places) {
                    expand(&path, segments, glob, marker, places, dev, found, seen);
                }
            }
        }
        Segment::Name(pieces) => {
            for (name, path, is_dir) in children {
                let chars: Vec<char> = name.chars().collect();
                if !name_matches(pieces, &chars) {
                    continue;
                }
                // The last part names the Match; any other, a directory to go into.
                if rest.is_empty() || (is_dir && !skipped(&dir_text, &name, glob, places)) {
                    expand(&path, rest, glob, marker, places, dev, found, seen);
                }
            }
        }
    }
}

/// A directory's entries: name, path, and whether it is a directory on the
/// same volume that may be gone into (a symlink is not).
fn children(dir: &Path, dev: u64) -> Vec<(String, PathBuf, bool)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let meta = std::fs::symlink_metadata(e.path()).ok()?;
            let is_dir = meta.is_dir() && meta.dev() == dev;
            Some((e.file_name().to_string_lossy().into_owned(), e.path(), is_dir))
        })
        .collect();
    out.sort();
    out
}

/// What is true of a Match's whole tree.
#[derive(Default)]
pub struct Tree {
    pub bytes: u64,
    pub newest: i64,
    pub foreign: bool,
    pub unreadable: bool,
    pub holds_repo: bool,
    pub other_volume: bool,
}

/// Reads every entry of the tree at `path`, not following symlinks.
pub fn tree(path: &Path, uid: u32, dev: u64) -> Tree {
    let mut t = Tree { newest: i64::MIN, ..Tree::default() };
    if path.file_name().is_some_and(|n| n == ".git") {
        t.holds_repo = true;
    }
    visit(path, uid, dev, &mut t);
    t
}

fn visit(path: &Path, uid: u32, dev: u64, t: &mut Tree) {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => {
            t.unreadable = true;
            return;
        }
    };
    t.bytes = t.bytes.saturating_add(meta.blocks().saturating_mul(512));
    t.newest = t.newest.max(meta.mtime()).max(meta.ctime());
    if meta.uid() != uid {
        t.foreign = true;
    }
    if meta.dev() != dev {
        t.other_volume = true;
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => {
            t.unreadable = true;
            return;
        }
    };
    for entry in entries {
        match entry {
            Ok(e) => {
                if e.file_name() == ".git" {
                    t.holds_repo = true;
                }
                visit(&e.path(), uid, dev, t);
            }
            Err(_) => t.unreadable = true,
        }
    }
}

/// Deletes a Match: a symlink or a file by itself, a directory with all it
/// holds (`remove_dir_all` does not follow symlinks inside it).
pub fn delete(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
}
