//! Walking a directory tree on several threads: stat-ing is what a walk
//! waits on, and the disk answers many at once.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

/// How many threads a walk uses.
pub fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 16)
}

/// Calls `visit` with every entry under `root` (not `root` itself), its
/// metadata read without following a symlink, on several threads at once.
/// `visit` answers whether to go into the entry when it is a directory.
/// `unreadable` is called for a directory that cannot be listed. The walk
/// ends early once `stop` is set.
pub fn walk<V, U>(root: &Path, stop: &AtomicBool, visit: V, unreadable: U)
where
    V: Fn(&Path, &Metadata) -> bool + Sync,
    U: Fn(&Path) + Sync,
{
    // The directories still to list, and how many threads are listing one.
    let queue = Mutex::new((vec![root.to_path_buf()], 0usize));
    let changed = Condvar::new();
    std::thread::scope(|scope| {
        for _ in 0..threads() {
            scope.spawn(|| {
                loop {
                    let dir = {
                        let mut q = queue.lock().unwrap();
                        loop {
                            if stop.load(Ordering::Relaxed) {
                                changed.notify_all();
                                return;
                            }
                            if let Some(dir) = q.0.pop() {
                                q.1 += 1;
                                break dir;
                            }
                            if q.1 == 0 {
                                changed.notify_all();
                                return;
                            }
                            q = changed.wait(q).unwrap();
                        }
                    };
                    let mut found: Vec<PathBuf> = Vec::new();
                    match std::fs::read_dir(&dir) {
                        Ok(entries) => {
                            for entry in entries {
                                if stop.load(Ordering::Relaxed) {
                                    break;
                                }
                                let Ok(entry) = entry else {
                                    unreadable(&dir);
                                    continue;
                                };
                                let path = entry.path();
                                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                                    unreadable(&path);
                                    continue;
                                };
                                if visit(&path, &meta) && meta.is_dir() {
                                    found.push(path);
                                }
                            }
                        }
                        Err(_) => unreadable(&dir),
                    }
                    let mut q = queue.lock().unwrap();
                    q.0.extend(found);
                    q.1 -= 1;
                    changed.notify_all();
                }
            });
        }
    });
}
