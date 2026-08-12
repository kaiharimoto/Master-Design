//! What the person currently has selected, and what they said about it.
//!
//! This is the live half of the visual bridge. When an agent is attached, `selection.get`
//! reads this file and learns three things at once: which nodes the user is looking at,
//! what they typed, and — if they drew on the canvas — a picture of what they meant.
//!
//! It is a file rather than an IPC channel on purpose. The studio and the agent are
//! separate processes with separate lifetimes; either can be restarted without the other
//! noticing, and the last thing the user pointed at is still there afterwards.

use crate::canonical::to_canonical_string;
use crate::error::Result;
use crate::id::NodeId;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Directory for state that belongs to a working session rather than to the design.
///
/// Hidden and git-ignored: two people working on the same project should not fight over
/// whose cursor is where.
pub const SESSION_DIR: &str = ".md";
pub const SELECTION_FILE: &str = "selection.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    /// Page the selection is on, by id or slug.
    #[serde(default)]
    pub page: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeId>,
    /// A note the user typed alongside the selection — "make this bounce".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Path, relative to the project, of a screenshot with the user's markup on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation: Option<String>,
    /// Viewport rectangle the user is looking at, in document units: `[x, y, w, h]`.
    ///
    /// Lets a snapshot show what *they* can see rather than the whole page, which for a
    /// long scrolling design is a very different picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<[f64; 4]>,
    #[serde(default)]
    pub updated_at: u64,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.note.is_empty()
    }
}

pub fn selection_path(project_dir: &Path) -> PathBuf {
    project_dir.join(SESSION_DIR).join(SELECTION_FILE)
}

/// Read the current selection. An absent file means "nothing selected", not an error —
/// the studio may simply not be running.
pub fn load_selection(project_dir: &Path) -> Selection {
    fs::read_to_string(selection_path(project_dir))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_selection(project_dir: &Path, selection: &Selection) -> Result<()> {
    let dir = project_dir.join(SESSION_DIR);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(SELECTION_FILE), to_canonical_string(selection)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("md-doc-selection")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_selection_round_trips() {
        let dir = tmpdir("roundtrip");
        let sel = Selection {
            page: "index".into(),
            nodes: vec![NodeId::from_static("nd_card")],
            note: "make this bounce".into(),
            annotation: Some(".md/scribble.png".into()),
            viewport: Some([0.0, 0.0, 1440.0, 900.0]),
            updated_at: 1_700_000_000_000,
        };
        save_selection(&dir, &sel).unwrap();

        let back = load_selection(&dir);
        assert_eq!(back.nodes, sel.nodes);
        assert_eq!(back.note, "make this bounce");
        assert_eq!(back.viewport, sel.viewport);
    }

    #[test]
    fn no_studio_running_means_an_empty_selection_not_a_failure() {
        let dir = tmpdir("absent");
        assert!(load_selection(&dir).is_empty());
    }

    #[test]
    fn a_corrupt_file_degrades_to_empty() {
        let dir = tmpdir("corrupt");
        fs::create_dir_all(dir.join(SESSION_DIR)).unwrap();
        fs::write(selection_path(&dir), "{ not json").unwrap();
        assert!(load_selection(&dir).is_empty());
    }
}
