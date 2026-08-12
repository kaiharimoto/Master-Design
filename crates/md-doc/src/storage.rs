//! The on-disk project format.
//!
//! A project is a **directory**, not a single file:
//!
//! ```text
//! my-site.mdproj/
//!   project.json      metadata, breakpoints, and the page index
//!   tokens.json       colours, type and spacing scales
//!   pages/*.json      one scene graph per page
//!   assets/           images and fonts
//!   animations/       project-local animation packages
//!   requests/         queued notes from the human to the AI
//! ```
//!
//! Splitting it up is what makes the format work for two editors at once. An AI editing
//! one page rewrites one file, so a diff shows the page that changed rather than a
//! single enormous blob. Assets stay out of the text entirely. And `requests/` is the
//! part that makes the mobile story work: there is no terminal on a phone to run an
//! agent in, so a note written on the phone lands in the project, travels through git,
//! and is picked up by a session on a desktop later.

use crate::canonical::to_canonical_string;
use crate::document::{Document, Page, ProjectMeta, Tokens};
use crate::error::{DocError, Result};
use crate::id::{NodeId, PageId};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = "project.json";
pub const TOKENS_FILE: &str = "tokens.json";
pub const PAGES_DIR: &str = "pages";
pub const ASSETS_DIR: &str = "assets";
pub const ANIMATIONS_DIR: &str = "animations";
pub const REQUESTS_DIR: &str = "requests";

/// `project.json` — everything except the scene graphs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFile {
    schema_version: u32,
    meta: ProjectMeta,
    pages: Vec<PageRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageRef {
    id: PageId,
    slug: String,
    file: String,
}

/// A note from the human to the AI, waiting to be picked up.
///
/// This is the asynchronous half of the visual bridge. The synchronous half is
/// `selection.get` over MCP, for when an agent is actually attached.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: String,
    /// Milliseconds since the Unix epoch. Deliberately not a formatted date: formatting
    /// is a display concern and a locale-dependent one.
    pub created_at: u64,
    /// What the person asked for, in their own words.
    pub note: String,
    /// Page the request refers to, by id or slug.
    pub page: String,
    /// Nodes selected when the note was written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection: Vec<NodeId>,
    /// Optional screenshot with the user's markup on it, relative to the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation: Option<String>,
    #[serde(default)]
    pub status: RequestStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RequestStatus {
    #[default]
    Open,
    Done,
    Dismissed,
}

/// Write a project directory, creating it if needed.
pub fn save_project(dir: &Path, doc: &Document) -> Result<()> {
    doc.validate()?;

    fs::create_dir_all(dir.join(PAGES_DIR))?;
    fs::create_dir_all(dir.join(ASSETS_DIR))?;

    let mut refs = Vec::with_capacity(doc.pages.len());
    let mut written: Vec<String> = Vec::new();

    for page in &doc.pages {
        let file = format!("{}.json", sanitize(&page.slug));
        refs.push(PageRef { id: page.id.clone(), slug: page.slug.clone(), file: file.clone() });
        write_atomic(&dir.join(PAGES_DIR).join(&file), &to_canonical_string(page)?)?;
        written.push(file);
    }

    let project =
        ProjectFile { schema_version: doc.schema_version, meta: doc.meta.clone(), pages: refs };
    write_atomic(&dir.join(PROJECT_FILE), &to_canonical_string(&project)?)?;

    let tokens_path = dir.join(TOKENS_FILE);
    if doc.tokens.is_empty() {
        let _ = fs::remove_file(&tokens_path);
    } else {
        write_atomic(&tokens_path, &to_canonical_string(&doc.tokens)?)?;
    }

    // A deleted page must not leave its file behind, or the next load would resurrect
    // nothing but the file would linger in git forever.
    prune_stale_pages(&dir.join(PAGES_DIR), &written)?;

    Ok(())
}

fn prune_stale_pages(pages_dir: &Path, keep: &[String]) -> Result<()> {
    let entries = match fs::read_dir(pages_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".json") && !keep.contains(&name) {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// Read a project directory.
pub fn load_project(dir: &Path) -> Result<Document> {
    let project_path = dir.join(PROJECT_FILE);
    let raw = fs::read_to_string(&project_path).map_err(|e| {
        DocError::Io(format!("could not read {}: {e}", project_path.display()))
    })?;
    let project: ProjectFile = serde_json::from_str(&raw)?;

    if project.schema_version > crate::document::SCHEMA_VERSION {
        return Err(DocError::Io(format!(
            "project uses schema version {} but this build understands up to {} — update Master Design",
            project.schema_version,
            crate::document::SCHEMA_VERSION
        )));
    }

    let mut pages = Vec::with_capacity(project.pages.len());
    for r in &project.pages {
        let path = dir.join(PAGES_DIR).join(&r.file);
        let raw = fs::read_to_string(&path)
            .map_err(|e| DocError::Io(format!("could not read {}: {e}", path.display())))?;
        let page: Page = serde_json::from_str(&raw)?;
        pages.push(page);
    }

    let tokens_path = dir.join(TOKENS_FILE);
    let tokens = if tokens_path.exists() {
        serde_json::from_str(&fs::read_to_string(&tokens_path)?)?
    } else {
        Tokens::default()
    };

    let doc =
        Document { schema_version: project.schema_version, meta: project.meta, tokens, pages };
    doc.validate()?;
    Ok(doc)
}

/// Does this directory look like a project?
pub fn is_project_dir(dir: &Path) -> bool {
    dir.join(PROJECT_FILE).is_file()
}

/// Search upward from a path for the project directory containing it.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_dir() { start.to_path_buf() } else { start.parent()?.to_path_buf() };
    loop {
        if is_project_dir(&cur) {
            return Some(cur);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Append a request to the queue.
pub fn save_request(dir: &Path, request: &Request) -> Result<PathBuf> {
    let requests = dir.join(REQUESTS_DIR);
    fs::create_dir_all(&requests)?;
    let path = requests.join(format!("{}.json", sanitize(&request.id)));
    write_atomic(&path, &to_canonical_string(request)?)?;
    Ok(path)
}

/// Read every queued request, oldest first.
pub fn load_requests(dir: &Path) -> Result<Vec<Request>> {
    let requests_dir = dir.join(REQUESTS_DIR);
    let entries = match fs::read_dir(&requests_dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // One malformed file should not hide the rest of the queue.
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(req) = serde_json::from_str::<Request>(&raw) {
                out.push(req);
            }
        }
    }
    out.sort_by_key(|r| r.created_at);
    Ok(out)
}

/// Milliseconds since the Unix epoch, for stamping a new request.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write via a temporary file and rename, so an interrupted save cannot leave a
/// half-written page behind. On a phone, "interrupted" is the operating system
/// reclaiming the app, which is routine rather than exceptional.
fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Reduce a slug to something safe to use as a filename on every platform we target.
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "page".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NodeId;
    use crate::node::{Node, NodeKind, RectGeometry};
    use crate::paint::Color;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("md-doc-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Document {
        let mut d = Document::new("Site");
        d.pages[0].root.id = NodeId::from_static("nd_root");
        d.pages[0].root.children.push(Node::new(
            NodeId::from_static("nd_card"),
            NodeKind::Rect(RectGeometry { width: 10.0, height: 10.0, corner_radius: [0.0; 4] }),
        ));
        d.tokens.colors.insert("accent".into(), Color::parse("#ff0055").unwrap());
        d
    }

    #[test]
    fn a_project_round_trips_through_disk() {
        let dir = tmpdir("roundtrip");
        let doc = sample();
        save_project(&dir, &doc).unwrap();
        let back = load_project(&dir).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn saving_is_byte_stable() {
        let dir = tmpdir("stable");
        let doc = sample();
        save_project(&dir, &doc).unwrap();
        let first = fs::read_to_string(dir.join(PAGES_DIR).join("index.json")).unwrap();

        let reloaded = load_project(&dir).unwrap();
        save_project(&dir, &reloaded).unwrap();
        let second = fs::read_to_string(dir.join(PAGES_DIR).join("index.json")).unwrap();

        assert_eq!(first, second, "a save/load/save cycle dirtied the file");
    }

    #[test]
    fn each_page_gets_its_own_file() {
        let dir = tmpdir("pages");
        let mut doc = sample();
        doc.pages.push(Page::new(PageId::from_static("pg_about"), "About", "about", 1440.0, 900.0));
        save_project(&dir, &doc).unwrap();

        assert!(dir.join(PAGES_DIR).join("index.json").exists());
        assert!(dir.join(PAGES_DIR).join("about.json").exists());
    }

    #[test]
    fn deleting_a_page_removes_its_file() {
        let dir = tmpdir("prune");
        let mut doc = sample();
        doc.pages.push(Page::new(PageId::from_static("pg_about"), "About", "about", 1440.0, 900.0));
        save_project(&dir, &doc).unwrap();
        assert!(dir.join(PAGES_DIR).join("about.json").exists());

        doc.pages.pop();
        save_project(&dir, &doc).unwrap();
        assert!(!dir.join(PAGES_DIR).join("about.json").exists(), "stale page file was left behind");
    }

    #[test]
    fn tokens_are_dropped_when_emptied() {
        let dir = tmpdir("tokens");
        let mut doc = sample();
        save_project(&dir, &doc).unwrap();
        assert!(dir.join(TOKENS_FILE).exists());

        doc.tokens.colors.clear();
        save_project(&dir, &doc).unwrap();
        assert!(!dir.join(TOKENS_FILE).exists());
    }

    #[test]
    fn a_newer_schema_is_refused_with_advice() {
        let dir = tmpdir("schema");
        let doc = sample();
        save_project(&dir, &doc).unwrap();

        let raw = fs::read_to_string(dir.join(PROJECT_FILE)).unwrap();
        fs::write(dir.join(PROJECT_FILE), raw.replace("\"schemaVersion\": 1", "\"schemaVersion\": 99"))
            .unwrap();

        let err = load_project(&dir).unwrap_err().to_string();
        assert!(err.contains("99") && err.contains("update"), "unhelpful error: {err}");
    }

    #[test]
    fn requests_queue_and_come_back_in_order() {
        let dir = tmpdir("requests");
        save_project(&dir, &sample()).unwrap();

        for (i, note) in ["make this bounce", "warmer accent"].iter().enumerate() {
            save_request(
                &dir,
                &Request {
                    id: format!("req_{i}"),
                    created_at: 1000 + i as u64,
                    note: note.to_string(),
                    page: "index".into(),
                    selection: vec![NodeId::from_static("nd_card")],
                    annotation: None,
                    status: RequestStatus::Open,
                },
            )
            .unwrap();
        }

        let out = load_requests(&dir).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].note, "make this bounce");
        assert_eq!(out[0].selection, vec![NodeId::from_static("nd_card")]);
    }

    #[test]
    fn an_empty_queue_is_not_an_error() {
        let dir = tmpdir("noqueue");
        assert!(load_requests(&dir).unwrap().is_empty());
    }

    #[test]
    fn project_roots_are_found_from_within() {
        let dir = tmpdir("find");
        save_project(&dir, &sample()).unwrap();
        let found = find_project_root(&dir.join(PAGES_DIR)).unwrap();
        assert_eq!(found, dir);
    }

    #[test]
    fn awkward_slugs_become_safe_filenames() {
        assert_eq!(sanitize("about/us"), "about-us");
        assert_eq!(sanitize("../../etc/passwd"), "etc-passwd");
        assert_eq!(sanitize(""), "page");
        assert_eq!(sanitize("///"), "page");
    }
}
