//! Discovering installed animation packages.
//!
//! The registry scans directories for folders containing a `manifest.json`. Later loads
//! shadow earlier ones by id, so the search order sets precedence:
//!
//! 1. the standard library shipped with the app
//! 2. packages installed by the user
//! 3. `animations/` inside the open project
//!
//! A project can therefore override a standard animation without editing anything
//! global, and the override travels with the project in git.
//!
//! This is the whole mechanism behind "keep adding animations forever": a new one is a
//! folder. No registration list to edit, no rebuild, no editor code — the parameter
//! specs in the manifest are what the inspector draws controls from.

use crate::error::{AnimError, Result};
use crate::manifest::{AnimationManifest, Category};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const PREVIEW_FILE: &str = "preview.svg";

/// A package as found on disk.
#[derive(Debug, Clone)]
pub struct AnimationPackage {
    pub manifest: AnimationManifest,
    pub dir: PathBuf,
    /// Where this came from, for the "installed by" column and for debugging why an
    /// override did or did not take effect.
    pub origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Standard,
    User,
    Project,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::Standard => "standard",
            Origin::User => "user",
            Origin::Project => "project",
        }
    }
}

impl AnimationPackage {
    /// Preview artwork for the picker, if the package ships any.
    pub fn preview_path(&self) -> Option<PathBuf> {
        let p = self.dir.join(PREVIEW_FILE);
        if p.is_file() {
            Some(p)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    packages: BTreeMap<String, AnimationPackage>,
    /// Directories that were scanned but could not be read, and why. Surfaced rather
    /// than swallowed: a typo'd path that silently yields no animations is a confusing
    /// thing to debug.
    pub problems: Vec<String>,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }

    /// Scan a directory of package folders. Returns how many were loaded.
    ///
    /// A malformed package is reported in [`Registry::problems`] and skipped — one bad
    /// folder must not take the whole library down.
    pub fn load_dir(&mut self, root: &Path, origin: Origin) -> usize {
        let entries = match fs::read_dir(root) {
            Ok(e) => e,
            Err(e) => {
                // A missing optional directory is normal, not a problem worth reporting.
                if root.exists() {
                    self.problems.push(format!("{}: {e}", root.display()));
                }
                return 0;
            }
        };

        let mut loaded = 0;
        let mut dirs: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        // Deterministic order, so two machines produce the same registry.
        dirs.sort();

        for dir in dirs {
            match load_package(&dir, origin) {
                Ok(pkg) => {
                    self.packages.insert(pkg.manifest.id.clone(), pkg);
                    loaded += 1;
                }
                Err(e) => self.problems.push(format!("{}: {e}", dir.display())),
            }
        }
        loaded
    }

    pub fn insert(&mut self, pkg: AnimationPackage) {
        self.packages.insert(pkg.manifest.id.clone(), pkg);
    }

    pub fn get(&self, id: &str) -> Option<&AnimationPackage> {
        self.packages.get(id)
    }

    pub fn require(&self, id: &str) -> Result<&AnimationPackage> {
        self.get(id).ok_or_else(|| AnimError::NotFound(id.to_string()))
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Every package, ordered by id.
    pub fn list(&self) -> Vec<&AnimationPackage> {
        self.packages.values().collect()
    }

    pub fn by_category(&self, category: Category) -> Vec<&AnimationPackage> {
        self.packages.values().filter(|p| p.manifest.category == category).collect()
    }

    /// Free-text search over id, title, description and tags.
    pub fn search(&self, query: &str) -> Vec<&AnimationPackage> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.list();
        }
        self.packages
            .values()
            .filter(|p| {
                let m = &p.manifest;
                m.id.to_lowercase().contains(&q)
                    || m.title.to_lowercase().contains(&q)
                    || m.description.to_lowercase().contains(&q)
                    || m.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .collect()
    }

    /// Packages that can be applied to the given node kinds.
    pub fn applicable_to(&self, kinds: &[&str]) -> Vec<&AnimationPackage> {
        self.packages
            .values()
            .filter(|p| kinds.iter().all(|k| p.manifest.applies_to.accepts_kind(k)))
            .collect()
    }
}

fn load_package(dir: &Path, origin: Origin) -> Result<AnimationPackage> {
    let path = dir.join(MANIFEST_FILE);
    let raw = fs::read_to_string(&path)
        .map_err(|e| AnimError::Io(format!("{}: {e}", path.display())))?;
    let manifest: AnimationManifest = serde_json::from_str(&raw)?;
    manifest.validate()?;
    Ok(AnimationPackage { manifest, dir: dir.to_path_buf(), origin })
}

/// Load the standard library, then user packages, then the open project's own — in the
/// order that makes each shadow the last.
pub fn load_default(
    standard: Option<&Path>,
    user: Option<&Path>,
    project: Option<&Path>,
) -> Registry {
    let mut reg = Registry::new();
    if let Some(p) = standard {
        reg.load_dir(p, Origin::Standard);
    }
    if let Some(p) = user {
        reg.load_dir(p, Origin::User);
    }
    if let Some(p) = project {
        reg.load_dir(&p.join(md_doc::storage::ANIMATIONS_DIR), Origin::Project);
    }
    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(root: &Path, folder: &str, id: &str, title: &str) {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).unwrap();
        let manifest = serde_json::json!({
            "id": id,
            "version": "1.0.0",
            "title": title,
            "description": "A test animation",
            "category": "entrance",
            "tags": ["test", "fade"],
            "defaultTrigger": { "type": "load" },
            "params": [
                { "key": "duration", "label": "Duration", "type": "number",
                  "default": 0.5, "min": 0.1, "max": 3.0 }
            ],
            "generator": {
                "kind": "declarative",
                "duration": "duration",
                "perTarget": {
                    "endAt": "duration",
                    "tracks": [{ "property": "opacity", "from": 0, "to": 1 }]
                }
            }
        });
        fs::write(dir.join(MANIFEST_FILE), serde_json::to_string_pretty(&manifest).unwrap())
            .unwrap();
    }

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("md-anim-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn packages_load_from_a_directory() {
        let root = tmpdir("load");
        write_package(&root, "fade-in", "std/fade-in", "Fade In");
        write_package(&root, "slide-up", "std/slide-up", "Slide Up");

        let mut reg = Registry::new();
        assert_eq!(reg.load_dir(&root, Origin::Standard), 2);
        assert_eq!(reg.len(), 2);
        assert!(reg.get("std/fade-in").is_some());
        assert!(reg.problems.is_empty());
    }

    #[test]
    fn a_project_package_shadows_the_standard_one() {
        let std_dir = tmpdir("shadow-std");
        let proj_dir = tmpdir("shadow-proj");
        write_package(&std_dir, "fade-in", "std/fade-in", "Fade In");
        write_package(&proj_dir, "fade-in", "std/fade-in", "Fade In (customised)");

        let mut reg = Registry::new();
        reg.load_dir(&std_dir, Origin::Standard);
        reg.load_dir(&proj_dir, Origin::Project);

        let pkg = reg.get("std/fade-in").unwrap();
        assert_eq!(pkg.manifest.title, "Fade In (customised)");
        assert_eq!(pkg.origin, Origin::Project);
        assert_eq!(reg.len(), 1, "an override should replace, not duplicate");
    }

    #[test]
    fn one_broken_package_does_not_take_down_the_rest() {
        let root = tmpdir("broken");
        write_package(&root, "good", "std/good", "Good");
        let bad = root.join("bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join(MANIFEST_FILE), "{ not json").unwrap();

        let mut reg = Registry::new();
        assert_eq!(reg.load_dir(&root, Origin::Standard), 1);
        assert_eq!(reg.problems.len(), 1);
        assert!(reg.problems[0].contains("bad"), "got {:?}", reg.problems);
    }

    #[test]
    fn a_missing_directory_is_quietly_empty() {
        let mut reg = Registry::new();
        assert_eq!(reg.load_dir(Path::new("/nonexistent/animations"), Origin::User), 0);
        assert!(reg.problems.is_empty(), "an absent optional directory is not a problem");
    }

    #[test]
    fn search_covers_title_and_tags() {
        let root = tmpdir("search");
        write_package(&root, "fade-in", "std/fade-in", "Fade In");
        write_package(&root, "draw", "std/draw-path", "Draw Path");

        let mut reg = Registry::new();
        reg.load_dir(&root, Origin::Standard);

        assert_eq!(reg.search("draw").len(), 1);
        assert_eq!(reg.search("fade").len(), 2, "both are tagged 'fade'");
        assert_eq!(reg.search("").len(), 2);
        assert_eq!(reg.search("nonsense").len(), 0);
    }

    #[test]
    fn applicable_to_filters_by_kind() {
        let root = tmpdir("applicable");
        write_package(&root, "fade-in", "std/fade-in", "Fade In");

        let mut reg = Registry::new();
        reg.load_dir(&root, Origin::Standard);
        assert_eq!(reg.applicable_to(&["text", "rect"]).len(), 1);
    }

    #[test]
    fn listing_is_deterministic() {
        let root = tmpdir("order");
        write_package(&root, "z", "std/z-last", "Z");
        write_package(&root, "a", "std/a-first", "A");

        let mut reg = Registry::new();
        reg.load_dir(&root, Origin::Standard);
        let ids: Vec<&str> = reg.list().iter().map(|p| p.manifest.id.as_str()).collect();
        assert_eq!(ids, vec!["std/a-first", "std/z-last"]);
    }
}
