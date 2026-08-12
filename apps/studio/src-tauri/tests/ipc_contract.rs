//! The frontend and the backend agree about what the commands are called.
//!
//! Tauri's IPC is stringly typed in both directions. `invoke("doc_snaphsot", …)` compiles
//! on the TypeScript side, compiles on the Rust side, and fails at runtime in front of a
//! user. So does passing `{ nodeId }` to a parameter Rust spells `node`, or forgetting an
//! argument entirely — that one arrives as a deserialization error mentioning a field
//! name the user has never heard of.
//!
//! Neither `tsc` nor `cargo` can see across that boundary, so this test does. It reads
//! `src/lib.rs` for the real command signatures and `../src/ipc.ts` for every call site,
//! and checks them against each other.
//!
//! It parses rather than reflects because the alternative — a build script emitting a
//! schema — puts a code generator between a developer and the error message, and the
//! thing being checked here is a couple of hundred lines of our own source in a style we
//! control.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Commands that no part of the interface calls yet.
///
/// Not a failure — several are geometry the tools will use as they land — but it is
/// checked, so that adding a command and forgetting to wire it up is a deliberate line in
/// this list rather than something nobody notices for a release or two.
const NOT_YET_CALLED: &[&str] = &[
    // Waiting on the polygon and star tools.
    "geom_polygon_path",
    "geom_star_path",
    // Waiting on the live-corners control in the properties panel.
    "geom_round_corners",
    // Waiting on "outline stroke" in the path menu.
    "geom_outline_stroke",
    // Waiting on the pencil tool.
    "geom_fit_freehand",
    // Waiting on "convert to path". Rectangles and ellipses are stored as live shapes and
    // only become path data at export, so nothing in the editor needs their outline yet.
    "geom_rect_path",
    "geom_ellipse_path",
    // Waiting on selector search — "select every node with the role `card`". The MCP
    // server already resolves selectors; this is the same thing with a text field in
    // front of it.
    "doc_query",
    // Waiting on preview mode and the animation picker's thumbnails.
    "doc_snapshot",
    // Waiting on an assets panel. Images arrive by being dropped, which goes through
    // `asset_import`; listing what is already in the project needs somewhere to show it.
    "asset_list",
];

fn studio_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// ---------------------------------------------------------------------------
// The Rust side
// ---------------------------------------------------------------------------

/// Arguments Tauri injects rather than reading from the message.
fn is_injected(ty: &str) -> bool {
    ty.contains("AppHandle") || ty.starts_with("State<") || ty.contains("Window")
}

#[derive(Debug)]
struct Command {
    /// Argument names as the webview must spell them: Tauri renames command parameters to
    /// camelCase on the JavaScript side.
    args: BTreeSet<String>,
    /// Arguments that may be omitted, because `Option<T>` deserializes from absent.
    optional: BTreeSet<String>,
}

fn to_camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_commands(source: &str) -> BTreeMap<String, Command> {
    let mut out = BTreeMap::new();

    for (offset, _) in source.match_indices("#[tauri::command]") {
        // Only where it is genuinely an attribute. This file's own module documentation
        // talks *about* `#[tauri::command]`, and taking that as a command left the parser
        // reading the next unrelated function.
        let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
        if !source[line_start..offset].chars().all(char::is_whitespace) {
            continue;
        }

        let rest = &source[offset..];
        let fn_at = rest
            .find("fn ")
            .expect("a command with no function after it");
        let after_fn = &rest[fn_at + 3..];
        let paren = after_fn
            .find('(')
            .expect("a command with no parameter list");
        let name = after_fn[..paren].trim().to_string();

        let params = balanced(&after_fn[paren..], '(', ')');

        let mut args = BTreeSet::new();
        let mut optional = BTreeSet::new();
        for param in split_top_level(params, ',') {
            let param = param.trim();
            if param.is_empty() {
                continue;
            }
            let Some((lhs, ty)) = param.split_once(':') else {
                continue;
            };
            let ty = ty.trim();
            if is_injected(ty) {
                continue;
            }
            let key = to_camel(lhs.trim());
            if ty.starts_with("Option<") {
                optional.insert(key.clone());
            }
            args.insert(key);
        }

        out.insert(name, Command { args, optional });
    }

    out
}

/// The text between a matching pair of delimiters, given a slice starting on the opener.
fn balanced(s: &str, open: char, close: char) -> &str {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return &s[open.len_utf8()..i];
            }
        }
    }
    panic!("unbalanced {open}{close} in: {}", &s[..s.len().min(120)]);
}

/// Split on a separator that is not nested inside brackets of any kind.
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

// ---------------------------------------------------------------------------
// The TypeScript side
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct CallSite {
    command: String,
    args: BTreeSet<String>,
    /// How the rest of the interface reaches this command: `project.open`, `geom.rectPath`.
    accessor: String,
}

fn parse_call_sites(source: &str) -> Vec<CallSite> {
    let mut out = Vec::new();

    for (offset, _) in source.match_indices("call<") {
        let rest = &source[offset..];
        // Past the type argument, whatever shape it is, to the actual call.
        let paren = rest.find('(').expect("call< with no argument list");
        let inside = balanced(&rest[paren..], '(', ')');

        let parts = split_top_level(inside, ',');
        let first = parts[0].trim();
        // `call` is also *declared* in this file, and its declaration looks enough like a
        // call site to be picked up. A real one names its command as a string literal.
        if !first.starts_with('"') {
            continue;
        }
        let command = first.trim_matches('"').to_string();

        let args = match parts.get(1) {
            Some(obj) => object_keys(obj.trim()),
            None => BTreeSet::new(),
        };

        out.push(CallSite {
            command,
            args,
            accessor: accessor_for(source, offset),
        });
    }

    out
}

/// Work out the `group.property` a call site sits inside.
///
/// Scanning backwards rather than parsing the module: the exported objects in `ipc.ts`
/// are written in one consistent shape — `export const group = {` at column zero and each
/// command a two-space-indented property — and a real TypeScript parser to read six
/// declarations would be a heavier dependency than the thing it checks.
fn accessor_for(source: &str, offset: usize) -> String {
    let before = &source[..offset];

    let group = before
        .rmatch_indices("export const ")
        .next()
        .map(|(i, _)| {
            let rest = &source[i + "export const ".len()..];
            rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_default();

    let property = before
        .lines()
        .rev()
        .find_map(|line| {
            let indented = line.strip_prefix("  ")?;
            if indented.starts_with(' ') || indented.starts_with('*') || indented.starts_with('/') {
                return None;
            }
            let name = indented.split(':').next()?.trim();
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then(|| name.to_string())
        })
        .unwrap_or_default();

    format!("{group}.{property}")
}

/// Top-level keys of an object literal, handling both `k: v` and the `k` shorthand.
fn object_keys(literal: &str) -> BTreeSet<String> {
    let literal = literal.trim();
    if !literal.starts_with('{') {
        return BTreeSet::new();
    }
    split_top_level(balanced(literal, '{', '}'), ',')
        .into_iter()
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let key = entry.split(':').next().unwrap_or(entry).trim();
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                .then(|| key.to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn commands() -> BTreeMap<String, Command> {
    parse_commands(&read(&studio_root().join("src/lib.rs")))
}

fn call_sites() -> Vec<CallSite> {
    parse_call_sites(&read(&studio_root().join("../src/ipc.ts")))
}

#[test]
fn the_parsers_understand_the_files_they_are_given() {
    // If either parser silently matched nothing, every check below would pass for the
    // wrong reason. This is the guard against a green test that checks nothing at all.
    let commands = commands();
    assert!(
        commands.len() >= 20,
        "only found {} commands in lib.rs — the parser has probably stopped matching",
        commands.len()
    );
    assert!(
        commands.contains_key("doc_patch"),
        "the parser missed doc_patch"
    );
    assert_eq!(
        commands["doc_snapshot"].args,
        ["atWidth", "nodeId", "page", "time", "width"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        "doc_snapshot's parameters did not come out as the webview must spell them"
    );

    let sites = call_sites();
    assert!(
        sites.len() >= 20,
        "only found {} call sites in ipc.ts",
        sites.len()
    );
}

#[test]
fn every_command_the_frontend_calls_exists() {
    let commands = commands();
    let missing: Vec<&str> = call_sites()
        .iter()
        .map(|s| s.command.as_str())
        .filter(|name| !commands.contains_key(*name))
        .map(|name| Box::leak(name.to_string().into_boxed_str()) as &str)
        .collect();

    assert!(
        missing.is_empty(),
        "ipc.ts calls commands the backend does not have: {missing:?}"
    );
}

#[test]
fn every_argument_the_frontend_sends_is_a_real_parameter() {
    let commands = commands();
    let mut problems = Vec::new();

    for site in call_sites() {
        let Some(command) = commands.get(&site.command) else {
            continue; // reported by the test above
        };
        for arg in &site.args {
            if !command.args.contains(arg) {
                problems.push(format!(
                    "{}: sends `{arg}`, which is not a parameter (it takes {:?})",
                    site.command, command.args
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_required_parameter_is_sent() {
    let commands = commands();
    let mut problems = Vec::new();

    for site in call_sites() {
        let Some(command) = commands.get(&site.command) else {
            continue;
        };
        for param in &command.args {
            if command.optional.contains(param) {
                // `Option<T>` deserializes from a missing key, so leaving it out is fine.
                continue;
            }
            if !site.args.contains(param) {
                problems.push(format!(
                    "{}: never sends `{param}`, so the call fails at runtime with a \
                     deserialization error",
                    site.command
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_command_has_a_typed_door_in_ipc_ts() {
    let called: BTreeSet<String> = call_sites().into_iter().map(|s| s.command).collect();

    let commands = commands();
    let orphaned: Vec<&String> = commands
        .keys()
        .filter(|name| !called.contains(*name))
        .collect();

    assert!(
        orphaned.is_empty(),
        "these commands exist in the backend but `ipc.ts` offers no way to call them, so \
         nothing in the interface can: {orphaned:?}"
    );
}

/// Everything under `apps/studio/src` except `ipc.ts` itself.
fn frontend_sources() -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("ts" | "tsx")
            ) && path.file_name().and_then(|n| n.to_str()) != Some("ipc.ts")
            {
                out.push(read(&path));
            }
        }
    }

    let mut out = Vec::new();
    walk(&studio_root().join("../src"), &mut out);
    assert!(!out.is_empty(), "found no frontend sources to search");
    // Whitespace stripped so `ipc.updates.check()` still matches when a formatter has
    // broken it across lines as `ipc.updates\n  .check()`.
    out.iter()
        .map(|s| s.split_whitespace().collect::<String>())
        .collect()
}

#[test]
fn no_command_is_quietly_unreachable() {
    let sources = frontend_sources();
    let expected_unused: BTreeSet<&str> = NOT_YET_CALLED.iter().copied().collect();

    let mut orphaned = Vec::new();
    let mut stale = Vec::new();

    for site in call_sites() {
        let used = sources.iter().any(|s| s.contains(&site.accessor));
        let listed = expected_unused.contains(site.command.as_str());

        if !used && !listed {
            orphaned.push(format!("{} (via {})", site.command, site.accessor));
        }
        if used && listed {
            stale.push(site.command);
        }
    }

    assert!(
        orphaned.is_empty(),
        "these commands are wired up in ipc.ts but nothing in the interface calls them — \
         finish the feature, or list them in NOT_YET_CALLED with a note about what they \
         are waiting for: {orphaned:#?}"
    );
    assert!(
        stale.is_empty(),
        "these are listed as not-yet-called but the interface now calls them, so the note \
         is out of date: {stale:?}"
    );
}

#[test]
fn the_registered_handler_list_matches_the_commands_that_exist() {
    // `generate_handler!` is a third place a command name is written, and a command that
    // is defined but not registered is invisible at runtime with no compile error.
    let source = read(&studio_root().join("src/lib.rs"));
    let start = source
        .find("tauri::generate_handler![")
        .expect("no generate_handler! in lib.rs");
    let list = balanced(
        &source[start + "tauri::generate_handler!".len()..],
        '[',
        ']',
    );

    let registered: BTreeSet<String> = list
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    let defined: BTreeSet<String> = commands().keys().cloned().collect();

    assert_eq!(
        defined
            .difference(&registered)
            .collect::<Vec<_>>()
            .as_slice(),
        &[] as &[&String],
        "defined but never registered, so the webview cannot call them"
    );
    assert_eq!(
        registered
            .difference(&defined)
            .collect::<Vec<_>>()
            .as_slice(),
        &[] as &[&String],
        "registered but not defined as commands"
    );
}
