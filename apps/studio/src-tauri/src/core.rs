//! Everything the studio's commands actually do.
//!
//! Separate from `lib.rs` for one reason: a `#[tauri::command]` cannot be called without
//! a Tauri runtime, a window and an event loop, so for as long as the behaviour lived
//! inside the command functions none of it could be tested at all. Roughly seven hundred
//! lines of document handling, saving, exporting and animation baking had exactly zero
//! tests, and the only way to exercise any of it was to build the app and click.
//!
//! So the rule is: **`lib.rs` translates, `core.rs` decides**. A command unwraps its
//! `State`, converts the odd `String` to a typed id, and calls one method here. Anything
//! with a branch in it belongs on this side of the line, where a test can reach it.
//!
//! Nothing in this file knows about Tauri, with one deliberate exception: [`Studio`] holds
//! an opaque `Box<dyn Any + Send>` for the filesystem watcher, because the watcher's
//! lifetime is exactly the open project's lifetime and parking it anywhere else would
//! mean the watcher for a closed project could outlive it.

use crate::watch::WatchGate;
use md_anim::Registry;
use md_doc::history::Session;
use md_doc::patch::Op;
use md_doc::storage::{self, Request, RequestStatus};
use md_doc::{Document, NodeId, Selector};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, String>;

/// An open project.
pub struct Studio {
    session: Session,
    project_dir: PathBuf,
    registry: Registry,
    /// Whether there are changes the file on disk does not have.
    dirty: bool,
    /// Shared with the watcher thread so a save can mark itself as ours without taking
    /// the editor lock the watcher would otherwise contend on.
    gate: Arc<WatchGate>,
    /// Dropping this stops the watcher, which is exactly what should happen when a
    /// different project is opened over the top of this one.
    watcher: Option<Box<dyn std::any::Any + Send>>,
}

/// What the frontend mirrors.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorState {
    pub document: Option<Document>,
    pub project_path: Option<String>,
    pub revision: u64,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    pub dirty: bool,
}

impl EditorState {
    /// The state with no project open — what the welcome screen renders from.
    pub fn empty() -> Self {
        EditorState {
            document: None,
            project_path: None,
            revision: 0,
            can_undo: false,
            can_redo: false,
            undo_label: None,
            redo_label: None,
            dirty: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub files: Vec<String>,
    pub bytes: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchResult {
    pub state: EditorState,
    pub report: md_doc::PatchReport,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInfo {
    pub manifest: md_anim::AnimationManifest,
    pub origin: &'static str,
}

// ---------------------------------------------------------------------------
// Opening and creating
// ---------------------------------------------------------------------------

impl Studio {
    /// Open an existing project.
    ///
    /// `path` may point at the project directory or at anything inside it — a page file
    /// dropped onto the window, say — because [`storage::find_project_root`] walks up.
    ///
    /// `standard_animations` is where the animations bundled with the app live. It is
    /// passed in rather than discovered here because finding it means asking Tauri for a
    /// resource directory, and that is precisely the kind of dependency this module
    /// exists to keep out.
    pub fn open(path: &Path, standard_animations: Option<&Path>) -> Result<Self> {
        let root = if storage::is_project_dir(path) {
            path.to_path_buf()
        } else {
            storage::find_project_root(path)
                .ok_or_else(|| format!("{} is not a Master Design project", path.display()))?
        };

        let doc = storage::load_project(&root).map_err(|e| e.to_string())?;
        let registry = load_registry(&root, standard_animations);

        Ok(Studio {
            session: Session::new(doc),
            project_dir: root,
            registry,
            dirty: false,
            gate: Arc::new(WatchGate::default()),
            watcher: None,
        })
    }

    /// Create a project and open it.
    pub fn create(
        path: &Path,
        name: &str,
        width: f64,
        height: f64,
        standard_animations: Option<&Path>,
        server_command: &str,
    ) -> Result<Self> {
        if storage::is_project_dir(path) {
            return Err(format!("{} already contains a project", path.display()));
        }

        let doc = Document::sized(name, width, height).map_err(|e| e.to_string())?;
        storage::scaffold_project(path, &doc, server_command).map_err(|e| e.to_string())?;

        Studio::open(path, standard_animations)
    }

    /// Re-read from disk, discarding unsaved changes and the undo history.
    ///
    /// This is how an edit made over MCP reaches the canvas. History is dropped because
    /// it describes a document that no longer exists — keeping it would let one Ctrl+Z
    /// reinstate a state the file has moved past.
    ///
    /// The watcher is deliberately carried across rather than restarted: the directory
    /// being watched has not changed, and tearing the watcher down and building a new one
    /// leaves a window in which an edit lands unseen.
    pub fn reload(&mut self, standard_animations: Option<&Path>) -> Result<()> {
        let doc = storage::load_project(&self.project_dir).map_err(|e| e.to_string())?;
        self.registry = load_registry(&self.project_dir, standard_animations);
        self.session = Session::new(doc);
        self.dirty = false;
        Ok(())
    }

    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    /// The open document, borrowed.
    ///
    /// Test-only on purpose. Every command hands the frontend an [`EditorState`], which
    /// carries a *clone*, so nothing outside can hold a reference into the live session
    /// and reason about a document that a patch may already have replaced.
    #[cfg(test)]
    pub fn document(&self) -> &Document {
        self.session.document()
    }

    /// The gate the watcher must share, so saves can identify themselves as ours.
    pub fn gate(&self) -> Arc<WatchGate> {
        self.gate.clone()
    }

    /// Park the watcher handle here so it lives exactly as long as the open project.
    pub fn attach_watcher(&mut self, handle: Box<dyn std::any::Any + Send>) {
        self.watcher = Some(handle);
    }

    pub fn state(&self) -> EditorState {
        let history = self.session.history();
        EditorState {
            document: Some(self.session.document().clone()),
            project_path: Some(self.project_dir.display().to_string()),
            revision: self.session.revision(),
            can_undo: history.can_undo(),
            can_redo: history.can_redo(),
            undo_label: history.undo_label().map(String::from),
            redo_label: history.redo_label().map(String::from),
            dirty: self.dirty,
        }
    }
}

/// The standard library first, then the project's own, so a project can shadow a bundled
/// animation by publishing a package with the same name.
fn load_registry(project: &Path, standard: Option<&Path>) -> Registry {
    let mut registry = Registry::new();
    if let Some(dir) = standard {
        registry.load_dir(dir, md_anim::Origin::Standard);
    }
    registry.load_dir(
        &project.join(storage::ANIMATIONS_DIR),
        md_anim::Origin::Project,
    );
    registry
}

// ---------------------------------------------------------------------------
// Saving
// ---------------------------------------------------------------------------

impl Studio {
    /// Write the project, marking the write as ours first.
    ///
    /// Every save goes through here. The marking is not optional: without it the watcher
    /// sees the studio's own save, reports a change, and the frontend reloads over a
    /// change the user just made.
    pub fn save(&mut self) -> Result<()> {
        self.gate.mark_self_write();
        storage::save_project(&self.project_dir, self.session.document())
            .map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

impl Studio {
    /// Apply operations as one undoable step, then save.
    ///
    /// Saved eagerly. The alternative — holding changes in memory until someone presses
    /// Save — would mean an attached AI reading a stale project off disk, and would lose
    /// work when the operating system reclaims the app on mobile.
    ///
    /// A save that fails after a successful apply is reported as an error, but the change
    /// stays applied and undoable: throwing away work the user can see on screen because
    /// the disk was full would be worse than telling them the disk is full.
    pub fn patch(&mut self, label: String, ops: Vec<Op>) -> Result<PatchResult> {
        let report = self.session.apply(label, ops).map_err(|e| e.to_string())?;
        self.dirty = true;

        if let Err(e) = self.save() {
            return Err(format!("the change applied but could not be saved: {e}"));
        }

        Ok(PatchResult {
            state: self.state(),
            report,
        })
    }

    pub fn undo(&mut self) -> Result<EditorState> {
        self.session.undo().map_err(|e| e.to_string())?;
        self.dirty = true;
        self.save()?;
        Ok(self.state())
    }

    pub fn redo(&mut self) -> Result<EditorState> {
        self.session.redo().map_err(|e| e.to_string())?;
        self.dirty = true;
        self.save()?;
        Ok(self.state())
    }

    pub fn query(&self, selector: &str, page: Option<&str>) -> Result<Vec<String>> {
        let parsed = Selector::parse(selector).map_err(|e| e.to_string())?;
        let doc = self.session.document();
        let ids = match page {
            Some(key) => parsed.select_in_page(doc, key).map_err(|e| e.to_string())?,
            None => parsed.select(doc),
        };
        Ok(ids.iter().map(|i| i.as_str().to_string()).collect())
    }

    /// Render a page to a base64 PNG.
    pub fn snapshot(
        &self,
        page: &str,
        time: Option<f64>,
        width: u32,
        node_id: Option<&str>,
    ) -> Result<String> {
        use base64::Engine;

        let opts = md_mcp::render::SnapshotOptions {
            width,
            time,
            node: node_id
                .map(NodeId::parse)
                .transpose()
                .map_err(|e| e.to_string())?,
            region: None,
        };
        let (png, _, _) = md_mcp::render::snapshot(self.session.document(), page, &opts)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(png))
    }

    pub fn export(&self, out: Option<PathBuf>) -> Result<ExportResult> {
        let target = out.unwrap_or_else(|| self.project_dir.join("dist"));

        let opts = md_emit::ExportOptions {
            assets_from: Some(self.project_dir.join(storage::ASSETS_DIR)),
            ..Default::default()
        };

        let report =
            md_emit::export(self.session.document(), &target, &opts).map_err(|e| e.to_string())?;

        Ok(ExportResult {
            files: report
                .files
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            bytes: report.bytes,
            warnings: report.warnings,
        })
    }
}

// ---------------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------------

impl Studio {
    pub fn animations(&self) -> Vec<PackageInfo> {
        self.registry
            .list()
            .into_iter()
            .map(|p| PackageInfo {
                manifest: p.manifest.clone(),
                origin: p.origin.as_str(),
            })
            .collect()
    }

    /// Bake a package against a selector without adding the result to the document.
    pub fn bake(
        &self,
        package: &str,
        selector: &str,
        params: serde_json::Map<String, serde_json::Value>,
        page: &str,
    ) -> Result<md_doc::Timeline> {
        md_anim::apply_to_selector(
            self.session.document(),
            &self.registry,
            package,
            selector,
            params,
            Some(page),
        )
        .map_err(|e| e.to_string())
    }

    /// Re-derive a timeline's keyframes after its parameters changed.
    ///
    /// The parameters are the truth and the keyframes are derived from them, so a change
    /// re-bakes rather than editing the keyframes in place. It also re-resolves the
    /// selector, which is what lets an animation pick up nodes added since it was applied.
    pub fn rebake(
        &self,
        timeline_id: &str,
        page: &str,
        params: serde_json::Map<String, serde_json::Value>,
    ) -> Result<md_doc::Timeline> {
        let doc = self.session.document();
        let existing = doc
            .page(page)
            .and_then(|p| p.timelines.iter().find(|t| t.id.as_str() == timeline_id))
            .ok_or_else(|| format!("no timeline {timeline_id} on page '{page}'"))?;

        let mut updated = existing.clone();
        if let Some(source) = updated.source.as_mut() {
            for (k, v) in params {
                source.params.insert(k, v);
            }
        } else {
            // A hand-authored timeline has no package behind it, so there is nothing to
            // re-derive from. Saying so beats returning the keyframes unchanged and
            // letting the user think their parameter did something.
            return Err(format!(
                "timeline {timeline_id} was not created from an animation package, so it has \
                 no parameters to re-bake"
            ));
        }

        md_anim::rebake(doc, &self.registry, &updated, Some(page)).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

impl Studio {
    /// Publish what the person is looking at, so an attached agent can answer "make this
    /// bounce" without being told what "this" is.
    pub fn publish_selection(
        &self,
        page: String,
        nodes: &[String],
        note: String,
        viewport: Option<[f64; 4]>,
    ) -> Result<()> {
        let selection = md_doc::Selection {
            page,
            nodes: parse_ids(nodes)?,
            note,
            annotation: None,
            viewport,
            updated_at: storage::now_millis(),
        };

        md_doc::selection::save_selection(&self.project_dir, &selection).map_err(|e| e.to_string())
    }

    /// Queue a note for a session that is not attached yet — the phone's path.
    pub fn queue_request(&self, note: String, page: String, nodes: &[String]) -> Result<String> {
        let created = storage::now_millis();
        let request = Request {
            id: format!("req_{created}"),
            created_at: created,
            note,
            page,
            selection: parse_ids(nodes)?,
            annotation: None,
            status: RequestStatus::Open,
        };

        storage::save_request(&self.project_dir, &request).map_err(|e| e.to_string())?;
        Ok(request.id)
    }

    /// The command line to attach an agent to this project, for the UI to show verbatim.
    pub fn mcp_command(&self) -> String {
        format!("md mcp {}", self.project_dir.display())
    }
}

fn parse_ids(nodes: &[String]) -> Result<Vec<NodeId>> {
    nodes
        .iter()
        .map(NodeId::parse)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("md-studio-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A project with one rect in it, and the rect's id.
    fn project(name: &str) -> (Studio, String) {
        let dir = tmpdir(name).join("site");
        let mut studio = Studio::create(&dir, "Test", 800.0, 600.0, None, "md").unwrap();

        let root = studio.document().pages[0].root.id.as_str().to_string();
        let ops: Vec<Op> = serde_json::from_value(json!([
            {"op": "node.insert", "parent": root, "node": {
                "id": "nd_card", "type": "rect", "roles": ["card"],
                "width": 200, "height": 100,
                "transform": [1, 0, 0, 1, 40, 40],
                "fills": [{"type": "solid", "color": "#ff0055"}]
            }}
        ]))
        .unwrap();
        studio.patch("Add a card".into(), ops).unwrap();
        (studio, "nd_card".to_string())
    }

    #[test]
    fn a_created_project_is_immediately_openable() {
        let dir = tmpdir("create").join("site");
        let studio = Studio::create(&dir, "My site", 1200.0, 800.0, None, "md").unwrap();

        assert_eq!(studio.document().meta.name, "My site");
        assert_eq!(studio.document().pages[0].width, 1200.0);
        assert!(dir.join("project.json").is_file());
        assert!(dir.join("animations").is_dir());
        assert!(
            dir.join(".mcp.json").is_file(),
            "a new project should be attachable with no setup"
        );
    }

    #[test]
    fn the_root_frame_matches_the_page_it_is_on() {
        // These are two separate fields and setting only one leaves everything laying out
        // against the default 1440×900 no matter what the page reports.
        let dir = tmpdir("root-frame").join("site");
        let studio = Studio::create(&dir, "Sized", 500.0, 400.0, None, "md").unwrap();
        let md_doc::NodeKind::Frame(frame) = &studio.document().pages[0].root.kind else {
            panic!("a page root should be a frame");
        };
        assert_eq!((frame.width, frame.height), (500.0, 400.0));
    }

    #[test]
    fn creating_over_an_existing_project_is_refused() {
        let dir = tmpdir("create-twice").join("site");
        Studio::create(&dir, "First", 800.0, 600.0, None, "md").unwrap();
        let second = Studio::create(&dir, "Second", 800.0, 600.0, None, "md");
        assert!(second.is_err(), "overwrote an existing project");
    }

    #[test]
    fn opening_a_file_inside_a_project_finds_the_project() {
        let dir = tmpdir("open-inner").join("site");
        Studio::create(&dir, "Test", 800.0, 600.0, None, "md").unwrap();

        let inner = dir.join("pages").join("index.json");
        let studio = Studio::open(&inner, None).unwrap();
        assert_eq!(studio.project_dir(), dir);
    }

    #[test]
    fn opening_something_that_is_not_a_project_says_so() {
        let dir = tmpdir("open-junk");
        let Err(err) = Studio::open(&dir, None) else {
            panic!("opened a directory that holds no project");
        };
        assert!(err.contains("not a Master Design project"), "got {err}");
    }

    #[test]
    fn a_patch_reaches_the_disk_without_anyone_pressing_save() {
        // The AI reads the project off disk. If edits sat in memory it would be reading
        // a document the user is no longer looking at.
        let (studio, _) = project("patch-saves");
        let reopened = Studio::open(studio.project_dir(), None).unwrap();
        assert!(
            reopened
                .document()
                .node(&NodeId::from_static("nd_card"))
                .is_some(),
            "the edit was not on disk"
        );
    }

    #[test]
    fn undo_and_redo_move_the_document_and_the_file_together() {
        let (mut studio, id) = project("undo-redo");
        let node = NodeId::from_static("nd_card");

        let state = studio.undo().unwrap();
        assert!(state.can_redo);
        assert!(studio.document().node(&node).is_none());
        assert!(
            Studio::open(studio.project_dir(), None)
                .unwrap()
                .document()
                .node(&node)
                .is_none(),
            "undo did not reach the disk, so an attached agent would still see {id}"
        );

        studio.redo().unwrap();
        assert!(studio.document().node(&node).is_some());
        assert!(Studio::open(studio.project_dir(), None)
            .unwrap()
            .document()
            .node(&node)
            .is_some());
    }

    #[test]
    fn undoing_with_nothing_to_undo_is_an_error_not_a_panic() {
        let dir = tmpdir("undo-empty").join("site");
        let mut studio = Studio::create(&dir, "Test", 800.0, 600.0, None, "md").unwrap();
        assert!(studio.undo().is_err());
        assert!(studio.redo().is_err());
    }

    #[test]
    fn an_invalid_patch_leaves_the_document_untouched() {
        let (mut studio, _) = project("bad-patch");
        let before = studio.document().clone();

        let ops: Vec<Op> = serde_json::from_value(json!([
            {"op": "node.delete", "id": "nd_card"},
            {"op": "node.delete", "id": "nd_does_not_exist"}
        ]))
        .unwrap();

        assert!(studio.patch("Remove two".into(), ops).is_err());
        assert_eq!(
            serde_json::to_string(studio.document()).unwrap(),
            serde_json::to_string(&before).unwrap(),
            "a partially applied patch left the document in a half-edited state"
        );
    }

    #[test]
    fn reload_picks_up_an_edit_made_underneath_us() {
        // This is the MCP path: something else wrote the project, and the studio has to
        // catch up without being restarted.
        let (mut studio, _) = project("reload");

        let mut doc = studio.document().clone();
        doc.meta.name = "Renamed by an agent".into();
        md_doc::storage::save_project(studio.project_dir(), &doc).unwrap();

        studio.reload(None).unwrap();
        assert_eq!(studio.document().meta.name, "Renamed by an agent");
        assert!(
            !studio.state().can_undo,
            "history survived a reload, so one Ctrl+Z would reinstate a state the file \
             has moved past"
        );
    }

    #[test]
    fn a_query_resolves_the_same_selectors_the_ai_uses() {
        let (studio, id) = project("query");
        assert_eq!(studio.query("@card", None).unwrap(), vec![id.clone()]);
        assert_eq!(studio.query("type:rect", Some("index")).unwrap(), vec![id]);
        assert!(studio.query("@nothing", None).unwrap().is_empty());
    }

    #[test]
    fn a_malformed_selector_is_reported_rather_than_matching_everything() {
        let (studio, _) = project("bad-selector");
        assert!(studio.query("type:", None).is_err());
    }

    #[test]
    fn exporting_writes_a_page_that_contains_the_artwork() {
        let (studio, _) = project("export");
        let out = studio.project_dir().join("dist");
        let report = studio.export(Some(out.clone())).unwrap();

        assert!(report.bytes > 0);
        let html = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(html.contains("nd_card"), "the exported page lost the card");
    }

    #[test]
    fn a_snapshot_is_a_real_png() {
        let (studio, _) = project("snapshot");
        let b64 = studio.snapshot("index", None, 320, None).unwrap();
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn a_snapshot_of_a_node_that_is_not_there_is_an_error() {
        let (studio, _) = project("snapshot-missing");
        assert!(studio
            .snapshot("index", None, 320, Some("nd_ghost"))
            .is_err());
    }

    #[test]
    fn publishing_a_selection_writes_something_the_bridge_can_read() {
        let (studio, id) = project("selection");
        studio
            .publish_selection(
                "index".into(),
                std::slice::from_ref(&id),
                "make this bounce".into(),
                Some([0.0, 0.0, 800.0, 600.0]),
            )
            .unwrap();

        let read = md_doc::selection::load_selection(studio.project_dir());
        assert_eq!(read.note, "make this bounce");
        assert_eq!(read.nodes[0].as_str(), id);
    }

    #[test]
    fn a_selection_naming_a_node_id_that_cannot_exist_is_refused() {
        // Better here than three layers down, where the error would arrive as a
        // deserialization failure with no mention of the selection.
        let (studio, _) = project("selection-bad-id");
        assert!(studio
            .publish_selection("index".into(), &["not an id".into()], String::new(), None)
            .is_err());
    }

    #[test]
    fn a_queued_request_survives_to_be_picked_up_later() {
        let (studio, id) = project("request");
        let request_id = studio
            .queue_request("add a hero image".into(), "index".into(), &[id])
            .unwrap();

        let queued = md_doc::storage::load_requests(studio.project_dir()).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, request_id);
        assert_eq!(queued[0].note, "add a hero image");
    }

    #[test]
    fn rebaking_a_hand_written_timeline_explains_itself() {
        let (studio, _) = project("rebake-handmade");
        let mut doc = studio.document().clone();
        doc.pages[0].timelines.push(md_doc::Timeline {
            id: md_doc::TimelineId::from_static("tl_hand"),
            name: "By hand".into(),
            trigger: md_doc::anim::Trigger::Load { delay: 0.0 },
            duration: 1.0,
            enabled: true,
            reduced_motion: Default::default(),
            source: None,
            tracks: vec![],
        });
        md_doc::storage::save_project(studio.project_dir(), &doc).unwrap();

        let mut studio = studio;
        studio.reload(None).unwrap();
        let err = studio
            .rebake("tl_hand", "index", Default::default())
            .unwrap_err();
        assert!(err.contains("animation package"), "got {err}");
    }

    #[test]
    fn rebaking_a_timeline_that_is_not_there_names_the_page() {
        let (studio, _) = project("rebake-missing");
        let err = studio
            .rebake("tl_ghost", "index", Default::default())
            .unwrap_err();
        assert!(
            err.contains("tl_ghost") && err.contains("index"),
            "got {err}"
        );
    }

    #[test]
    fn a_project_with_no_animation_library_still_opens() {
        // `standard_animations` is `None` whenever the resource directory is missing,
        // which is the normal case in a development checkout run from the wrong folder.
        let (studio, _) = project("no-registry");
        assert!(studio.animations().is_empty());
    }

    #[test]
    fn the_mcp_command_names_this_project() {
        let (studio, _) = project("mcp-command");
        let cmd = studio.mcp_command();
        assert!(cmd.starts_with("md mcp "));
        assert!(cmd.contains(&studio.project_dir().display().to_string()));
    }
}
