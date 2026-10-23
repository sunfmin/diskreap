use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// A day and a bit, in seconds: what the clock is moved on by so that
/// what a test wrote is old enough for a Rule of a day.
pub const LATER: i64 = 90_000;

pub struct Env {
    _dir: tempfile::TempDir,
    pub home: PathBuf,
    pub tmpdir: PathBuf,
    pub var_folder: PathBuf,
    bin: PathBuf,
    state: PathBuf,
    pub now: Option<i64>,
    /// The size scan reports from, when not a gigabyte.
    pub large: Option<u64>,
}

/// What a run printed: its answer on stdout, its events on stderr.
pub struct Out {
    pub code: i32,
    pub answer: Value,
    pub events: Vec<Value>,
    pub stdout: String,
}

fn standin() -> PathBuf {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_diskreap"));
    exe.parent().unwrap().join("examples/standin")
}

impl Env {
    pub fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        let var_folder = root.join("var");
        let tmpdir = var_folder.join("T");
        let bin = root.join("bin");
        for d in [&home, &tmpdir, &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        for name in ["gh", "lsof", "ps", "mdfind"] {
            std::os::unix::fs::symlink(standin(), bin.join(name)).unwrap();
        }
        let state = root.join("standin.json");
        std::fs::write(&state, "{}").unwrap();
        Env { _dir: dir, home, tmpdir, var_folder, bin, state, now: None, large: None }
    }

    /// An Env whose gist holds these Rules.
    pub fn with_rules(rules: Value) -> Env {
        let env = Env::new();
        let content = serde_json::to_string_pretty(&json!({ "rules": rules })).unwrap();
        env.set("gists", json!({"g1": {"content": content, "version": 1}}));
        env
    }

    pub fn state(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(&self.state).unwrap()).unwrap()
    }

    pub fn set(&self, key: &str, value: Value) {
        let mut s = self.state();
        s[key] = value;
        std::fs::write(&self.state, s.to_string()).unwrap();
    }

    /// The Rule list the gist holds now.
    pub fn gist_rules(&self) -> Value {
        let s = self.state();
        let content = s["gists"]["g1"]["content"].as_str().unwrap().to_string();
        serde_json::from_str::<Value>(&content).unwrap()["rules"].clone()
    }

    pub fn run(&self, args: &[&str]) -> Out {
        self.run_in(args, None)
    }

    pub fn run_in(&self, args: &[&str], stdin: Option<&str>) -> Out {
        let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display());
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_diskreap"));
        cmd.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmpdir)
            .env("PATH", path)
            .env("DISKREAP_STANDIN", &self.state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(now) = self.now {
            cmd.env("DISKREAP_NOW", now.to_string());
        }
        if let Some(large) = self.large {
            cmd.env("DISKREAP_LARGE", large.to_string());
        }
        let mut child = cmd.spawn().unwrap();
        {
            let mut i = child.stdin.take().unwrap();
            if let Some(text) = stdin {
                i.write_all(text.as_bytes()).unwrap();
            }
        }
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let answer = serde_json::from_str(&stdout).unwrap_or(Value::Null);
        let events = stderr
            .lines()
            .map(|l| {
                serde_json::from_str(l).unwrap_or_else(|_| panic!("an event that is not JSON: {l}"))
            })
            .collect();
        Out { code: out.status.code().unwrap_or(-1), answer, events, stdout }
    }

    /// Runs with the clock moved on past a day.
    pub fn later(&mut self) -> &mut Env {
        self.now = Some(now() + LATER);
        self
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// Writes a file, making its directories.
pub fn file(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
        .arg(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

pub fn cargo_target_rule() -> Value {
    json!({"id": "cargo-target", "glob": "~/**/target", "marker": "Cargo.toml", "why": "cargo builds it again"})
}
