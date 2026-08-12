//! Finding the things a command needs that were not passed to it.

use std::path::{Path, PathBuf};

/// Locate the standard animation library.
///
/// Checked in order, most explicit first:
///
/// 1. an explicit `--animations` flag
/// 2. `MD_ANIMATIONS` in the environment
/// 3. `animations/` beside the executable — how a shipped build is laid out
/// 4. `packages/md-anim-std/animations` up the tree from the current directory — how a
///    checkout is laid out, so the CLI works from a `cargo run` without any setup
///
/// Returning `None` is survivable: everything except the animation commands works
/// without a library, and saying so beats refusing to start.
pub fn standard_animations(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_dir().then(|| p.to_path_buf());
    }

    if let Ok(env) = std::env::var("MD_ANIMATIONS") {
        let p = PathBuf::from(env);
        if p.is_dir() {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("animations");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }

    let mut cursor = std::env::current_dir().ok()?;
    loop {
        let candidate = cursor.join("packages/md-anim-std/animations");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !cursor.pop() {
            return None;
        }
    }
}

/// Resolve a project argument, accepting either the project directory itself or any
/// path inside it.
pub fn project_root(path: &Path) -> anyhow::Result<PathBuf> {
    if md_doc::storage::is_project_dir(path) {
        return Ok(path.to_path_buf());
    }
    md_doc::storage::find_project_root(path).ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not a Master Design project and none was found above it \
             (looking for a directory containing project.json)",
            path.display()
        )
    })
}
