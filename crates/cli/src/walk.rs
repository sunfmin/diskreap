//! Finding a Rule's Matches on disk, and reading what is true of each one's
//! tree, on several threads. Nothing follows a symlink, and nothing goes onto
//! another volume.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};

use diskreap_rules::{Glob, Places, Segment, name_matches, path_matches, reached, skipped};

use crate::par;

/// A glob to find, with the marker its Matches need, by its place in the run.
pub type Wanted<'a> = (usize, &'a Glob, Option<&'a str>);

/// Hands each path a glob matches that has its marker beside it to `found`,
/// with the glob's place, as soon as it is found and on this thread, not
/// looking inside a Match for more. The walks run on other threads
/// meanwhile; globs that search any depth from the same directory share one.
pub fn matches(globs: &[Wanted], places: &Places, dev: u64, found: &mut dyn FnMut(usize, PathBuf)) {
    // A base reached through a symlink is not where it says it is.
    let real: Vec<Wanted> = globs
        .iter()
        .filter(|(_, g, _)| {
            let base = Path::new(&g.base);
            std::fs::canonicalize(base).is_ok_and(|r| r == base)
        })
        .copied()
        .collect();
    let mut shared: Vec<Vec<Wanted>> = Vec::new();
    let mut alone: Vec<Wanted> = Vec::new();
    for w in real {
        if w.1.segments.first() != Some(&Segment::AnyDepth) {
            alone.push(w);
        } else if let Some(group) = shared.iter_mut().find(|g| g[0].1.base == w.1.base) {
            group.push(w);
        } else {
            shared.push(vec![w]);
        }
    }
    let (send, receive) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            for group in shared {
                let base = PathBuf::from(&group[0].1.base);
                Find { wanted: group, places, dev, send: send.clone() }.any_depth(&base);
            }
            for w in alone {
                let base = PathBuf::from(&w.1.base);
                Find { wanted: vec![w], places, dev, send: send.clone() }
                    .expand(&base, &w.1.segments);
            }
        });
        for (i, path) in receive {
            found(i, path);
        }
    });
}

/// Hands `path` to `found` once for each glob a walk would find it as a Match
/// of: the glob reaches it, its marker is beside it, and every directory
/// from the glob's base down to it is on the home volume. `path` is real:
/// no symlink and no `.` or `..` in it.
pub fn at(
    globs: &[Wanted],
    places: &Places,
    dev: u64,
    path: &Path,
    found: &mut dyn FnMut(usize, PathBuf),
) {
    let text = path.to_string_lossy();
    for &(i, glob, marker) in globs {
        let base = Path::new(&glob.base);
        if !std::fs::canonicalize(base).is_ok_and(|r| r == base)
            || !reached(glob, &text, places)
            || !Find::marked(marker, path)
        {
            continue;
        }
        let on_volume = path
            .ancestors()
            .skip(1)
            .take_while(|d| d.starts_with(base))
            .all(|d| std::fs::symlink_metadata(d).is_ok_and(|m| m.dev() == dev));
        if on_volume {
            found(i, path.to_path_buf());
        }
    }
}

/// One walk: the globs it looks for, all with the same base.
struct Find<'a> {
    wanted: Vec<Wanted<'a>>,
    places: &'a Places,
    dev: u64,
    send: mpsc::Sender<(usize, PathBuf)>,
}

impl Find<'_> {
    /// The glob a walk alone follows, and whether its directories are skipped.
    fn first(&self) -> &Glob {
        self.wanted[0].1
    }

    fn marked(marker: Option<&str>, path: &Path) -> bool {
        marker.is_none_or(|m| path.parent().is_some_and(|p| p.join(m).exists()))
    }

    fn skipped(&self, path: &Path) -> bool {
        let parent = path.parent().map(|p| p.to_string_lossy()).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        skipped(&parent, &name, self.first(), self.places)
    }

    /// For a walk of one glob: the parts before its first `**` are followed
    /// name by name; from a `**` on, the tree is walked in parallel.
    fn expand(&self, dir: &Path, segments: &[Segment]) {
        let (i, _, marker) = self.wanted[0];
        let Some((segment, rest)) = segments.split_first() else {
            if std::fs::symlink_metadata(dir).is_ok() && Find::marked(marker, dir) {
                let _ = self.send.send((i, dir.to_path_buf()));
            }
            return;
        };
        let Segment::Name(pieces) = segment else {
            self.any_depth(dir);
            return;
        };
        for (name, path, is_dir) in children(dir, self.dev) {
            let chars: Vec<char> = name.chars().collect();
            if !name_matches(pieces, &chars) {
                continue;
            }
            // The last part names the Match; any other, a directory to go into.
            if rest.is_empty() || (is_dir && !self.skipped(&path)) {
                self.expand(&path, rest);
            }
        }
    }

    /// Walks the tree under `dir` in parallel, testing each path against
    /// every glob wanted; a `**` matches no directory as well, so the walk
    /// starts at `dir`'s own entries.
    fn any_depth(&self, dir: &Path) {
        let stop = AtomicBool::new(false);
        let send = Mutex::new(self.send.clone());
        par::walk(
            dir,
            &stop,
            |path, meta| {
                let text = path.to_string_lossy();
                let mut matched = false;
                for &(i, glob, marker) in &self.wanted {
                    if path_matches(glob, &text) && Find::marked(marker, path) {
                        let _ = send.lock().unwrap().send((i, path.to_path_buf()));
                        matched = true;
                    }
                }
                !matched && meta.is_dir() && meta.dev() == self.dev && !self.skipped(path)
            },
            |_| {},
        );
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

/// How much later than its access time a file may have been read last.
pub const READ_LAG: i64 = 86_400;

/// What is true of a Match's tree.
#[derive(Default)]
pub struct Tree {
    /// Allocated bytes of what was read: all of it when `complete`.
    pub bytes: u64,
    pub newest: i64,
    pub foreign: bool,
    pub unreadable: bool,
    pub holds_repo: bool,
    pub other_volume: bool,
    /// Every entry was read. A walk given a cutoff stops at the first entry
    /// that holds the Match, since nothing read after it could free it.
    pub complete: bool,
}

/// Reads the tree at `path`, not following symlinks. With a cutoff, it stops
/// as soon as anything in it changed after the cutoff, belongs to another
/// user, is on another volume, is a `.git` or cannot be read.
pub fn tree(path: &Path, uid: u32, dev: u64, cutoff: Option<i64>) -> Tree {
    let Ok(root) = std::fs::symlink_metadata(path) else {
        return Tree { newest: i64::MIN, unreadable: true, ..Tree::default() };
    };
    let bytes = AtomicU64::new(0);
    let newest = AtomicI64::new(i64::MIN);
    let [foreign, unreadable, holds_repo, other_volume, stop] =
        [(); 5].map(|_| AtomicBool::new(false));
    let held = |flag: &AtomicBool| {
        flag.store(true, Ordering::Relaxed);
        if cutoff.is_some() {
            stop.store(true, Ordering::Relaxed);
        }
    };
    // What one entry says; whether to go into it.
    let see = |path: &Path, meta: &std::fs::Metadata| -> bool {
        bytes.fetch_add(meta.blocks().saturating_mul(512), Ordering::Relaxed);
        // A file's last read is its access time, which macOS moves on only
        // when it is older than the file's changes or than a day (as Linux's
        // relatime): a file read just now may show a read up to a day old,
        // so a read counts as a day later than it says. A directory's access
        // time is not use: listing it, as this walk does, moves it. Where the
        // access time cannot be read (a volume that keeps none says 0), the
        // writes alone count: whichever is newest is the last use.
        let atime = meta.accessed().ok().map(|_| meta.atime()).filter(|&t| t > 0);
        let read = match atime {
            Some(t) if !meta.is_dir() => t.saturating_add(READ_LAG),
            _ => i64::MIN,
        };
        let time = meta.mtime().max(meta.ctime()).max(read);
        newest.fetch_max(time, Ordering::Relaxed);
        if cutoff.is_some_and(|c| time > c) {
            stop.store(true, Ordering::Relaxed);
        }
        if meta.uid() != uid {
            held(&foreign);
        }
        if path.file_name().is_some_and(|n| n == ".git") {
            held(&holds_repo);
        }
        if meta.dev() != dev {
            held(&other_volume);
            return false;
        }
        meta.is_dir()
    };
    if see(path, &root) && !stop.load(Ordering::Relaxed) {
        par::walk(path, &stop, see, |_| held(&unreadable));
    }
    Tree {
        bytes: bytes.into_inner(),
        newest: newest.into_inner(),
        foreign: foreign.into_inner(),
        unreadable: unreadable.into_inner(),
        holds_repo: holds_repo.into_inner(),
        other_volume: other_volume.into_inner(),
        complete: !stop.into_inner(),
    }
}

/// Deletes a Match: a symlink or a file by itself, a directory with all it
/// holds (`remove_dir_all` does not follow symlinks inside it). On APFS, listing
/// a large directory while its entries go can miss some, and the directory
/// is then not empty at the end: it is gone over again, a few times, so a
/// Match is not left half deleted.
pub fn delete(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return std::fs::remove_file(path);
    }
    let mut tries = 0;
    loop {
        match std::fs::remove_dir_all(path) {
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty && tries < 5 => tries += 1,
            done => return done,
        }
    }
}
