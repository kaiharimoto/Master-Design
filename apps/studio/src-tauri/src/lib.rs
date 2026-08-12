//! The studio's backend.
//!
//! The document lives here, not in the webview. Every change — a mouse drag, a menu
//! command, a patch that arrived over MCP — goes through the same `md-doc` operations,
//! the same validator and the same undo stack. The frontend holds a reactive mirror and
//! asks; it never edits its copy directly.
//!
//! That is the whole reason this file is thin. Almost every command below is a few
//! lines of translation over a core crate, because the core crates are where the
//! behaviour is, and because those crates are also what `md-cli` and the MCP server use.
//! One implementation, three front ends.

mod updater;
mod watch;

use md_anim::Registry;
use md_doc::history::Session;
use md_doc::patch::Op;
use md_doc::storage::{self, Request, RequestStatus};
use md_doc::{Document, NodeId, Selector};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};

/// Everything an open project needs.
struct Studio {
    session: Session,
    project_dir: PathBuf,
    registry: Registry,
    /// Whether there are changes the file on disk does not have.
    dirty: bool,
    /// Dropping this stops the watcher, which is exactly what should happen when a
    /// different project is opened over the top of this one.
    _watcher: Option<Box<dyn std::any::Any + Send>>,
}

#[derive(Default)]
struct AppState {
    studio: Mutex<Option<Studio>>,
    /// Shared with the watcher thread so a save can mark itself as ours without taking
    /// the editor lock the watcher would otherwise contend on.
    gate: Arc<watch::WatchGate>,
}

/// What the frontend mirrors.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorState {
    document: Option<Document>,
    project_path: Option<String>,
    revision: u64,
    can_undo: bool,
    can_redo: bool,
    undo_label: Option<String>,
    redo_label: Option<String>,
    dirty: bool,
}

impl EditorState {
    fn empty() -> Self {
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

    fn of(studio: &Studio) -> Self {
        let history = studio.session.history();
        EditorState {
            document: Some(studio.session.document().clone()),
            project_path: Some(studio.project_dir.display().to_string()),
            revision: studio.session.revision(),
            can_undo: history.can_undo(),
            can_redo: history.can_redo(),
            undo_label: history.undo_label().map(String::from),
            redo_label: history.redo_label().map(String::from),
            dirty: studio.dirty,
        }
    }
}

type Result<T> = std::result::Result<T, String>;

/// Write the project, marking the write as ours first.
///
/// Every save goes through here. The marking is not optional: without it the watcher
/// sees the studio's own save, reloads, and throws away the undo history for a change
/// the user just made.
fn save(studio: &Studio, gate: &watch::WatchGate) -> Result<()> {
    gate.mark_self_write();
    storage::save_project(&studio.project_dir, studio.session.document()).map_err(|e| e.to_string())
}

fn with_studio<T>(state: &State<AppState>, f: impl FnOnce(&mut Studio) -> Result<T>) -> Result<T> {
    let mut guard = state
        .studio
        .lock()
        .map_err(|_| "editor state is poisoned".to_string())?;
    let studio = guard.as_mut().ok_or("no project is open")?;
    f(studio)
}

/// Where the animations that ship with the app live.
///
/// Beside the executable in a packaged build; up the tree from the working directory in
/// a checkout, so `tauri dev` finds them with no setup.
fn standard_animations(app: &tauri::AppHandle) -> Option<PathBuf> {
    if let Ok(dir) = app.path().resource_dir() {
        let candidate = dir.join("animations");
        if candidate.is_dir() {
            return Some(candidate);
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

fn load_registry(app: &tauri::AppHandle, project: &Path) -> Registry {
    let mut registry = Registry::new();
    if let Some(std_dir) = standard_animations(app) {
        registry.load_dir(&std_dir, md_anim::Origin::Standard);
    }
    registry.load_dir(
        &project.join(storage::ANIMATIONS_DIR),
        md_anim::Origin::Project,
    );
    registry
}

// ---------------------------------------------------------------------------
// Project
// ---------------------------------------------------------------------------

#[tauri::command]
fn editor_state(state: State<AppState>) -> Result<EditorState> {
    let guard = state
        .studio
        .lock()
        .map_err(|_| "editor state is poisoned".to_string())?;
    Ok(guard
        .as_ref()
        .map(EditorState::of)
        .unwrap_or_else(EditorState::empty))
}

#[tauri::command]
fn project_open(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: String,
) -> Result<EditorState> {
    let dir = PathBuf::from(&path);
    let root = if storage::is_project_dir(&dir) {
        dir
    } else {
        storage::find_project_root(&dir)
            .ok_or_else(|| format!("{path} is not a Master Design project"))?
    };

    let doc = storage::load_project(&root).map_err(|e| e.to_string())?;
    let registry = load_registry(&app, &root);

    // Started before the state is swapped in, so there is no window in which the
    // project is open but unwatched.
    let watcher = match watch::watch_project(app.clone(), root.clone(), state.gate.clone()) {
        Ok(handle) => Some(handle),
        Err(e) => {
            // Watching is a convenience, not a precondition for editing. Losing it means
            // AI edits need a manual reload, which is worth a warning and not a refusal.
            eprintln!("could not watch {}: {e}", root.display());
            None
        }
    };

    let studio = Studio {
        session: Session::new(doc),
        project_dir: root,
        registry,
        dirty: false,
        _watcher: watcher,
    };
    let snapshot = EditorState::of(&studio);
    *state
        .studio
        .lock()
        .map_err(|_| "editor state is poisoned".to_string())? = Some(studio);
    Ok(snapshot)
}

#[tauri::command]
fn project_create(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: String,
    name: String,
    width: f64,
    height: f64,
) -> Result<EditorState> {
    let root = PathBuf::from(&path);
    if storage::is_project_dir(&root) {
        return Err(format!("{path} already contains a project"));
    }

    let mut doc = Document::new(&name);
    doc.pages[0].width = width;
    doc.pages[0].height = height;
    if let md_doc::NodeKind::Frame(frame) = &mut doc.pages[0].root.kind {
        frame.width = width;
        frame.height = height;
    }
    doc.pages[0].background = Some(md_doc::Paint::solid("#ffffff").map_err(|e| e.to_string())?);

    storage::save_project(&root, &doc).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(root.join(storage::ANIMATIONS_DIR)).map_err(|e| e.to_string())?;

    project_open(app, state, root.display().to_string())
}

#[tauri::command]
fn project_save(state: State<AppState>) -> Result<EditorState> {
    let gate = state.gate.clone();
    with_studio(&state, |studio| {
        save(studio, &gate)?;
        studio.dirty = false;
        Ok(EditorState::of(studio))
    })
}

/// Re-read from disk, discarding unsaved changes.
///
/// This is how an edit made over MCP reaches the canvas: the server writes the project,
/// the studio reloads it. Undo history is dropped because it describes a document that
/// no longer exists — keeping it would let one Ctrl+Z reinstate a state the file has
/// moved past.
#[tauri::command]
fn project_reload(app: tauri::AppHandle, state: State<AppState>) -> Result<EditorState> {
    let path = with_studio(&state, |studio| {
        Ok(studio.project_dir.display().to_string())
    })?;
    project_open(app, state, path)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResult {
    files: Vec<String>,
    bytes: usize,
    warnings: Vec<String>,
}

#[tauri::command]
fn project_export(state: State<AppState>, out: Option<String>) -> Result<ExportResult> {
    with_studio(&state, |studio| {
        let target = out
            .map(PathBuf::from)
            .unwrap_or_else(|| studio.project_dir.join("dist"));

        let opts = md_emit::ExportOptions {
            assets_from: Some(studio.project_dir.join(storage::ASSETS_DIR)),
            ..Default::default()
        };

        let report = md_emit::export(studio.session.document(), &target, &opts)
            .map_err(|e| e.to_string())?;

        Ok(ExportResult {
            files: report
                .files
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            bytes: report.bytes,
            warnings: report.warnings,
        })
    })
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchResult {
    state: EditorState,
    report: md_doc::PatchReport,
}

#[tauri::command]
fn doc_patch(state: State<AppState>, ops: Vec<Op>, label: String) -> Result<PatchResult> {
    let gate = state.gate.clone();
    with_studio(&state, |studio| {
        let report = studio
            .session
            .apply(label, ops)
            .map_err(|e| e.to_string())?;
        studio.dirty = true;

        // Saved eagerly. The alternative — holding changes in memory until someone
        // presses Save — would mean an attached AI reading a stale project off disk,
        // and would lose work when the operating system reclaims the app on mobile.
        if let Err(e) = save(studio, &gate) {
            return Err(format!("the change applied but could not be saved: {e}"));
        }
        studio.dirty = false;

        Ok(PatchResult {
            state: EditorState::of(studio),
            report,
        })
    })
}

#[tauri::command]
fn doc_undo(state: State<AppState>) -> Result<EditorState> {
    let gate = state.gate.clone();
    with_studio(&state, |studio| {
        studio.session.undo().map_err(|e| e.to_string())?;
        let _ = save(studio, &gate);
        Ok(EditorState::of(studio))
    })
}

#[tauri::command]
fn doc_redo(state: State<AppState>) -> Result<EditorState> {
    let gate = state.gate.clone();
    with_studio(&state, |studio| {
        studio.session.redo().map_err(|e| e.to_string())?;
        let _ = save(studio, &gate);
        Ok(EditorState::of(studio))
    })
}

#[tauri::command]
fn doc_query(
    state: State<AppState>,
    selector: String,
    page: Option<String>,
) -> Result<Vec<String>> {
    with_studio(&state, |studio| {
        let parsed = Selector::parse(&selector).map_err(|e| e.to_string())?;
        let doc = studio.session.document();
        let ids = match page {
            Some(key) => parsed
                .select_in_page(doc, &key)
                .map_err(|e| e.to_string())?,
            None => parsed.select(doc),
        };
        Ok(ids.iter().map(|i| i.as_str().to_string()).collect())
    })
}

/// Mint ids in the backend so they match the document's format exactly.
///
/// The frontend could generate a ULID itself, but then two places would define what an
/// id looks like, and a subtle mismatch would only surface when `NodeId::parse` rejected
/// something the user had already drawn.
#[tauri::command]
fn doc_new_ids(count: usize) -> Vec<String> {
    (0..count.clamp(1, 512))
        .map(|_| NodeId::new().as_str().to_string())
        .collect()
}

#[tauri::command]
fn doc_snapshot(
    state: State<AppState>,
    page: String,
    time: Option<f64>,
    width: u32,
    node_id: Option<String>,
) -> Result<String> {
    use base64::Engine;
    with_studio(&state, |studio| {
        let opts = md_mcp::render::SnapshotOptions {
            width,
            time,
            node: node_id
                .map(NodeId::parse)
                .transpose()
                .map_err(|e| e.to_string())?,
            region: None,
        };
        let (png, _, _) = md_mcp::render::snapshot(studio.session.document(), &page, &opts)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(png))
    })
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[tauri::command]
fn geom_rect_path(w: f64, h: f64, radii: Vec<f64>) -> String {
    let r = [
        radii.first().copied().unwrap_or(0.0),
        radii.get(1).copied().unwrap_or(0.0),
        radii.get(2).copied().unwrap_or(0.0),
        radii.get(3).copied().unwrap_or(0.0),
    ];
    md_geom::rect_path(0.0, 0.0, w, h, r)
}

#[tauri::command]
fn geom_ellipse_path(w: f64, h: f64) -> String {
    md_geom::ellipse_path(w / 2.0, h / 2.0, w / 2.0, h / 2.0)
}

#[tauri::command]
fn geom_polygon_path(radius: f64, sides: u32, rotation: f64) -> String {
    md_geom::polygon_path(radius, radius, radius, sides, rotation)
}

#[tauri::command]
fn geom_star_path(outer: f64, inner: f64, points: u32, rotation: f64) -> String {
    md_geom::star_path(outer, outer, outer, inner, points, rotation)
}

#[tauri::command]
fn geom_round_corners(d: String, radius: f64) -> Result<String> {
    md_geom::round_corners(&d, radius).map_err(|e| e.to_string())
}

#[tauri::command]
fn geom_outline_stroke(d: String, width: f64) -> Result<String> {
    let style = md_geom::StrokeStyle {
        width,
        ..Default::default()
    };
    md_geom::outline_stroke(&d, &style).map_err(|e| e.to_string())
}

#[tauri::command]
fn geom_fit_freehand(points: Vec<f64>, tolerance: f64) -> Result<String> {
    if points.len() % 2 != 0 {
        return Err("points must be a flat [x, y, x, y, …] array".into());
    }
    let pts: Vec<[f64; 2]> = points.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
    let tolerance = if tolerance > 0.0 {
        tolerance
    } else {
        md_geom::DEFAULT_TOLERANCE
    };
    let simplified = md_geom::simplify_rdp(&pts, tolerance);
    Ok(md_geom::to_svg(&md_geom::fit_cubics(
        &simplified,
        tolerance * 2.0,
    )))
}

// ---------------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PackageInfo {
    manifest: md_anim::AnimationManifest,
    origin: &'static str,
}

#[tauri::command]
fn anim_list(state: State<AppState>) -> Result<Vec<PackageInfo>> {
    with_studio(&state, |studio| {
        Ok(studio
            .registry
            .list()
            .into_iter()
            .map(|p| PackageInfo {
                manifest: p.manifest.clone(),
                origin: p.origin.as_str(),
            })
            .collect())
    })
}

#[tauri::command]
fn anim_bake(
    state: State<AppState>,
    package: String,
    selector: String,
    params: serde_json::Map<String, serde_json::Value>,
    page: String,
) -> Result<md_doc::Timeline> {
    with_studio(&state, |studio| {
        md_anim::apply_to_selector(
            studio.session.document(),
            &studio.registry,
            &package,
            &selector,
            params,
            Some(&page),
        )
        .map_err(|e| e.to_string())
    })
}

/// Re-derive a timeline's keyframes after its parameters changed.
///
/// The parameters are the truth and the keyframes are derived from them, so a change
/// re-bakes rather than editing the keyframes in place. It also re-resolves the
/// selector, which is what lets an animation pick up nodes added since it was applied.
#[tauri::command]
fn anim_rebake(
    state: State<AppState>,
    timeline_id: String,
    page: String,
    params: serde_json::Map<String, serde_json::Value>,
) -> Result<md_doc::Timeline> {
    with_studio(&state, |studio| {
        let doc = studio.session.document();
        let existing = doc
            .page(&page)
            .and_then(|p| p.timelines.iter().find(|t| t.id.as_str() == timeline_id))
            .ok_or_else(|| format!("no timeline {timeline_id} on page '{page}'"))?;

        let mut updated = existing.clone();
        if let Some(source) = updated.source.as_mut() {
            for (k, v) in params {
                source.params.insert(k, v);
            }
        }

        md_anim::rebake(doc, &studio.registry, &updated, Some(&page)).map_err(|e| e.to_string())
    })
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

#[tauri::command]
fn selection_publish(
    state: State<AppState>,
    page: String,
    nodes: Vec<String>,
    note: String,
    viewport: Option<Vec<f64>>,
) -> Result<()> {
    with_studio(&state, |studio| {
        let ids: std::result::Result<Vec<NodeId>, _> = nodes.iter().map(NodeId::parse).collect();

        let selection = md_doc::Selection {
            page,
            nodes: ids.map_err(|e| e.to_string())?,
            note,
            annotation: None,
            viewport: viewport.and_then(|v| {
                if v.len() == 4 {
                    Some([v[0], v[1], v[2], v[3]])
                } else {
                    None
                }
            }),
            updated_at: storage::now_millis(),
        };

        md_doc::selection::save_selection(&studio.project_dir, &selection)
            .map_err(|e| e.to_string())
    })
}

#[tauri::command]
fn request_queue(
    state: State<AppState>,
    note: String,
    page: String,
    nodes: Vec<String>,
) -> Result<String> {
    with_studio(&state, |studio| {
        let ids: std::result::Result<Vec<NodeId>, _> = nodes.iter().map(NodeId::parse).collect();

        let created = storage::now_millis();
        let request = Request {
            id: format!("req_{created}"),
            created_at: created,
            note,
            page,
            selection: ids.map_err(|e| e.to_string())?,
            annotation: None,
            status: RequestStatus::Open,
        };

        storage::save_request(&studio.project_dir, &request).map_err(|e| e.to_string())?;
        Ok(request.id)
    })
}

/// The command line to attach an agent to this project, for the UI to show verbatim.
#[tauri::command]
fn mcp_command(state: State<AppState>) -> Result<String> {
    with_studio(&state, |studio| {
        Ok(format!("md mcp {}", studio.project_dir.display()))
    })
}

// ---------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------

#[tauri::command]
async fn update_check(app: tauri::AppHandle) -> Result<updater::UpdateInfo> {
    updater::check(app).await
}

#[tauri::command]
async fn update_install(app: tauri::AppHandle) -> Result<()> {
    updater::install(app).await
}

#[tauri::command]
fn update_open_release_page(app: tauri::AppHandle) -> Result<()> {
    updater::open_release_page(&app)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    builder
        .invoke_handler(tauri::generate_handler![
            editor_state,
            project_open,
            project_create,
            project_save,
            project_reload,
            project_export,
            doc_patch,
            doc_undo,
            doc_redo,
            doc_query,
            doc_new_ids,
            doc_snapshot,
            geom_rect_path,
            geom_ellipse_path,
            geom_polygon_path,
            geom_star_path,
            geom_round_corners,
            geom_outline_stroke,
            geom_fit_freehand,
            anim_list,
            anim_bake,
            anim_rebake,
            selection_publish,
            request_queue,
            mcp_command,
            update_check,
            update_install,
            update_open_release_page,
        ])
        .setup(|app| {
            // `md-studio --project <dir>` opens a project straight away. Useful on its
            // own, and it is what lets the screenshot harness photograph a real document
            // rather than the welcome screen.
            if let Some(path) = project_argument() {
                let handle = app.handle().clone();
                let state = app.state::<AppState>();
                if let Err(e) = project_open(handle, state, path.clone()) {
                    // Not fatal: the welcome screen is a perfectly good place to land,
                    // and refusing to start because one path was wrong would be worse
                    // than showing it.
                    eprintln!("could not open {path}: {e}");
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        // A plugin whose configuration is missing or malformed fails here, and the
        // default panic prints a backtrace that buries the one line that matters.
        .unwrap_or_else(|e| {
            eprintln!("Master Design could not start: {e}");
            std::process::exit(1);
        });
}

/// Read `--project <dir>` (or `--project=<dir>`) from the command line.
fn project_argument() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if let Some(rest) = args[i].strip_prefix("--project=") {
            return Some(rest.to_string());
        }
        if args[i] == "--project" {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}
