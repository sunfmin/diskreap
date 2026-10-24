//! What every command reads from the environment first: the places a glob's
//! variables stand for, the clock, and who and where the user is.

use std::os::unix::fs::MetadataExt;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use diskreap_rules::Places;

pub struct World {
    pub places: Places,
    /// Seconds since 1970.
    pub now: i64,
    /// The user: the owner of the home directory.
    pub uid: u32,
    /// The volume the home directory is on.
    pub dev: u64,
}

pub fn read() -> Result<World> {
    let home = std::env::var("HOME")
        .map_err(|_| anyhow!("HOME is not set: run diskreap from the user's login shell"))?;
    let tmpdir = std::env::var("TMPDIR").map_err(|_| {
        anyhow!("TMPDIR is not set: run diskreap from the user's login shell, or `TMPDIR=$(getconf DARWIN_USER_TEMP_DIR) diskreap …`")
    })?;
    // The real paths: `/var` is a link to `/private/var`, and a Match is
    // found by its real path.
    let home = std::fs::canonicalize(&home)
        .with_context(|| format!("the home directory {home} cannot be read"))?;
    let tmpdir = std::fs::canonicalize(&tmpdir)
        .with_context(|| format!("the temporary directory {tmpdir} cannot be read"))?;
    let meta = std::fs::metadata(&home)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    // Tests move the clock forward: a file's status-change time cannot be set
    // back. Only a debug build reads it; a release cannot be told the time.
    #[cfg(debug_assertions)]
    let now = std::env::var("DISKREAP_NOW").ok().and_then(|n| n.parse().ok()).unwrap_or(now);
    Ok(World {
        places: Places::new(&home.to_string_lossy(), &tmpdir.to_string_lossy()),
        now,
        uid: meta.uid(),
        dev: meta.dev(),
    })
}
