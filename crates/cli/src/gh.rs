//! The gist the Rule list is kept in, reached through `gh`: the only place
//! that builds a `gh` command line.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};
use diskreap_rules::{FILE, GIST_DESCRIPTION, Places, Rule, list_text, parse_list};
use serde_json::{Value, json};

/// The gist as read: its id, the version it was at, and the Rule list's text.
pub struct Gist {
    pub id: String,
    pub version: String,
    pub text: String,
}

fn gh(args: &[&str], input: Option<&str>) -> Result<String> {
    let mut child = Command::new("gh")
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            anyhow!(
                "gh cannot be run ({e}): install it with `brew install gh`, then `gh auth login`"
            )
        })?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let fix = if said.contains("auth") || said.contains("login") || said.contains("401") {
            "log gh in with `gh auth login` (it needs the gist scope)"
        } else {
            "check `gh auth status` and the network, then run it again"
        };
        bail!("`gh {}` failed: {said}: {fix}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The ids of the user's gists that hold the Rule list: among the hundred
/// updated last, and only when none is there among all of them, since
/// listing every page takes seconds.
pub fn find() -> Result<Vec<String>> {
    let jq = format!(".[] | select(.files[\"{FILE}\"]) | .id");
    let ids = |out: String| -> Vec<String> {
        out.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
    };
    let recent = ids(gh(&["api", "/gists?per_page=100", "--jq", &jq], None)?);
    if !recent.is_empty() {
        return Ok(recent);
    }
    Ok(ids(gh(&["api", "--paginate", "/gists?per_page=100", "--jq", &jq], None)?))
}

/// Where the id of the gist found last is kept, so a run need not list
/// every gist: `~/.config/diskreap/gist`. It is only a shortcut; a gist it
/// names that is gone or holds no Rule list is looked for again.
pub fn cache() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/diskreap/gist"))
}

fn remember(id: &str) {
    if let Some(path) = cache() {
        let _ = path.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(path, format!("{id}\n"));
    }
}

/// The gist holding the Rule list, as it is now: the one the cache names
/// when it still holds the list, or else the one found among your gists.
pub fn open() -> Result<Gist> {
    let cached = cache().and_then(|p| std::fs::read_to_string(p).ok());
    if let Some(id) = cached.as_deref().map(str::trim).filter(|id| !id.is_empty())
        && let Ok(gist) = read(id)
    {
        return Ok(gist);
    }
    let gist = read(&the_gist()?)?;
    remember(&gist.id);
    Ok(gist)
}

/// The one gist that holds the Rule list, looked for among all of yours.
fn the_gist() -> Result<String> {
    let ids = find()?;
    match ids.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!("no gist of yours holds {FILE}: run `diskreap init` to make it"),
        _ => bail!(
            "several gists hold {FILE} ({}): keep one, delete the others with `gh gist delete <id>`",
            ids.join(", ")
        ),
    }
}

pub fn read(id: &str) -> Result<Gist> {
    let out = gh(&["api", &format!("/gists/{id}")], None)?;
    let value: Value = serde_json::from_str(&out).map_err(|e| {
        anyhow!("gh answered the gist {id} with what is not JSON ({e}): run it again")
    })?;
    let text = value["files"][FILE]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("the gist {id} has no {FILE}: run `diskreap init` again"))?
        .to_string();
    let version = value["history"][0]["version"].as_str().unwrap_or_default().to_string();
    Ok(Gist { id: id.to_string(), version, text })
}

/// Makes the gist, private, with an empty Rule list; its id.
pub fn create() -> Result<String> {
    let body = json!({
        "description": GIST_DESCRIPTION,
        "public": false,
        "files": { FILE: { "content": list_text(&[]) } },
    });
    let out = gh(&["api", "-X", "POST", "/gists", "--input", "-"], Some(&body.to_string()))?;
    let value: Value = serde_json::from_str(&out).unwrap_or_default();
    value["id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow!("gh made a gist and did not say its id: run `diskreap init` again"))
        .inspect(|id| remember(id))
}

/// The Rule list as the gist holds it now.
pub fn load(places: &Places) -> Result<(Gist, Vec<Rule>)> {
    let gist = open()?;
    let rules = parse_list(&gist.text, places).map_err(|faults| {
        anyhow!("the Rule list in the gist {} is wrong: {}", gist.id, faults.join("; "))
    })?;
    Ok((gist, rules))
}

/// Writes the Rule list, unless the gist changed since it was read.
pub fn save(gist: &Gist, rules: &[Rule]) -> Result<()> {
    let now = read(&gist.id)?;
    if now.version != gist.version {
        bail!(
            "the gist {} changed while this ran (another diskreap, or a hand edit): run the command again",
            gist.id
        );
    }
    let body = json!({ "files": { FILE: { "content": list_text(rules) } } });
    gh(
        &["api", "-X", "PATCH", &format!("/gists/{}", gist.id), "--input", "-"],
        Some(&body.to_string()),
    )?;
    Ok(())
}

/// Whether gh is installed and logged in, as `doctor` reports it.
pub fn status() -> Result<(), String> {
    gh(&["auth", "status"], None).map(|_| ()).map_err(|e| e.to_string())
}
