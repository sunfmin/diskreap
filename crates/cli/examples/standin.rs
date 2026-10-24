//! Stands in for `gh`, `lsof`, `ps` and `mdfind` in the tests: linked under
//! each name first on PATH, it answers as that command from one JSON file
//! named by `DISKREAP_STANDIN`, records each call there, and fails when told.
//!
//! The file: `gists` {id: {"content": text, "version": n}}, `open` [paths
//! lsof lists], `running` [paths ps lists], `bundles` [ids mdfind finds],
//! `fail` {command: message}, `bump_on_read` (each gist read moves its
//! version, as another writer would), `calls` [[command, args...]].

use std::io::Read;

use serde_json::{Value, json};

const FILE: &str = "diskreap-rules.json";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args[0].rsplit('/').next().unwrap_or("").to_string();
    let rest = &args[1..];
    let path = std::env::var("DISKREAP_STANDIN").expect("DISKREAP_STANDIN");
    let mut state: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into()))
            .unwrap();
    let mut call = vec![json!(name)];
    call.extend(rest.iter().map(|a| json!(a)));
    if !state["calls"].is_array() {
        state["calls"] = json!([]);
    }
    state["calls"].as_array_mut().unwrap().push(Value::Array(call));
    let failure = state["fail"][&name].as_str().map(String::from);
    let (code, out) = match failure {
        Some(message) => {
            eprintln!("{message}");
            (1, String::new())
        }
        None => answer(&name, rest, &mut state),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&state).unwrap()).unwrap();
    print!("{out}");
    std::process::exit(code);
}

fn answer(name: &str, args: &[String], state: &mut Value) -> (i32, String) {
    let list = |key: &str| -> Vec<String> {
        state[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    };
    match name {
        "lsof" => {
            let mut out = String::from("p1\nn/dev/null\n");
            for p in list("open") {
                out.push_str(&format!("p2\nn{p}\n"));
            }
            (0, out)
        }
        "ps" => (0, list("running").iter().map(|p| format!("{p}\n")).collect()),
        "mdfind" => {
            let query = args.first().cloned().unwrap_or_default();
            let found = list("bundles").into_iter().find(|b| query.contains(&format!("\"{b}\"")));
            (0, found.map(|b| format!("/Applications/{b}.app\n")).unwrap_or_default())
        }
        "gh" => gh(args, state),
        _ => (127, String::new()),
    }
}

fn gh(args: &[String], state: &mut Value) -> (i32, String) {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    if !state["gists"].is_object() {
        state["gists"] = json!({});
    }
    match a.as_slice() {
        ["auth", "status"] => (0, String::new()),
        ["api", "--paginate", "/gists?per_page=100", "--jq", _]
        | ["api", "/gists?per_page=100", "--jq", _] => {
            let ids: Vec<String> = state["gists"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(_, g)| g["content"].is_string())
                .map(|(id, _)| format!("{id}\n"))
                .collect();
            (0, ids.concat())
        }
        ["api", path] if path.starts_with("/gists/") => {
            let id = &path["/gists/".len()..];
            let bump = state["bump_on_read"] == true;
            let Some(g) = state["gists"].get_mut(id) else { return (1, String::new()) };
            if bump {
                g["version"] = json!(g["version"].as_u64().unwrap_or(0) + 1);
            }
            let out = json!({
                "id": id,
                "files": { FILE: { "content": g["content"] } },
                "history": [{ "version": format!("v{}", g["version"]) }],
            });
            (0, out.to_string())
        }
        ["api", "-X", method, path, "--input", "-"] => {
            let mut body = String::new();
            std::io::stdin().read_to_string(&mut body).unwrap();
            let body: Value = serde_json::from_str(&body).unwrap();
            let content = body["files"][FILE]["content"].clone();
            match *method {
                "POST" => {
                    assert_eq!(body["public"], false, "a gist is made private");
                    let id = format!("g{}", state["gists"].as_object().unwrap().len() + 1);
                    state["gists"][&id] = json!({"content": content, "version": 1});
                    (0, json!({"id": id}).to_string())
                }
                "PATCH" => {
                    let id = &path["/gists/".len()..];
                    let g = &mut state["gists"][id];
                    g["content"] = content;
                    g["version"] = json!(g["version"].as_u64().unwrap_or(0) + 1);
                    (0, "{}".into())
                }
                _ => (1, String::new()),
            }
        }
        _ => {
            eprintln!("standin gh: no answer for {args:?}");
            (1, String::new())
        }
    }
}
