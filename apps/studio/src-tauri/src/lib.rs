//! The studio's Tauri surface.
//!
//! The document lives in the backend, not in the webview. Every change — a mouse drag, a
//! menu command, a patch that arrived over MCP — goes through the same `md-doc`
//! operations, the same validator and the same undo stack. The frontend holds a reactive
//! mirror and asks; it never edits its copy directly.
//!
//! This file is deliberately dull. Each command takes its lock, converts whatever the
//! webview sent into typed values, and calls one method on [`core::Studio`]. The reason
//! for the split is testability: a `#[tauri::command]` needs a runtime and a window to
//! run at all, so anything that lives inside one is unreachable from a test. If you are
//! about to write a branch in this file, it belongs in `core.rs`.
//!
//! The exceptions are the three things that are genuinely Tauri's: resolving the bundled
//! resource directory, starting the filesystem watcher, and the updater.

mod core;
mod updater;
mod watch;

use crate::core::{EditorState, ExportResult, PackageInfo, PatchResult, Studio};
use md_doc::patch::Op;
use md_doc::NodeId;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{Manager, State};

#[derive(Default)]
struct AppState {
    studio: Mutex<Option<Studio>>,
}

type Result<T> = std::result::Result<T, String>;

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

/// What to put in a new project's `.mcp.json` as the command that starts the bridge.
///
/// A packaged studio ships the `md` binary beside itself, and naming it by absolute path
/// means an agent attaches with nothing installed. Failing that we fall back to the bare
/// name, which works whenever the CLI is on `PATH` — and when it is neither, the file is
/// still a correct description of what to run, which is better than no file at all.
fn mcp_server_command(app: &tauri::AppHandle) -> String {
    let exe_name = if cfg!(windows) { "md.exe" } else { "md" };

    let beside_executable = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(exe_name)));
    let in_resources = app.path().resource_dir().ok().map(|d| d.join(exe_name));

    [beside_executable, in_resources]
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "md".to_string())
}

/// Open a project into the app state, watcher and all.
///
/// Shared by the `project_open` command, `project_create`, and the `--project` argument,
/// because "a project is now open" has to mean the same thing however it happened —
/// including being watched, which was the part most likely to be forgotten.
fn open_into_state(
    app: &tauri::AppHandle,
    state: &State<AppState>,
    path: &Path,
) -> Result<EditorState> {
    let mut studio = Studio::open(path, standard_animations(app).as_deref())?;

    // Attached before the state is swapped in, so there is no window in which the project
    // is open but unwatched.
    match watch::watch_project(
        app.clone(),
        studio.project_dir().to_path_buf(),
        studio.gate(),
    ) {
        Ok(handle) => studio.attach_watcher(handle),
        // Watching is a convenience, not a precondition for editing. Losing it means AI
        // edits need a manual reload, which is worth a warning and not a refusal.
        Err(e) => eprintln!("could not watch {}: {e}", studio.project_dir().display()),
    }

    let snapshot = studio.state();
    *state
        .studio
        .lock()
        .map_err(|_| "editor state is poisoned".to_string())? = Some(studio);
    Ok(snapshot)
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
        .map(Studio::state)
        .unwrap_or_else(EditorState::empty))
}

#[tauri::command]
fn project_open(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: String,
) -> Result<EditorState> {
    open_into_state(&app, &state, Path::new(&path))
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
    // Created and immediately dropped: `open_into_state` reopens it with a watcher
    // attached, so there is exactly one code path that puts a project into app state.
    Studio::create(
        &root,
        &name,
        width,
        height,
        standard_animations(&app).as_deref(),
        &mcp_server_command(&app),
    )?;
    open_into_state(&app, &state, &root)
}

#[tauri::command]
fn project_save(state: State<AppState>) -> Result<EditorState> {
    with_studio(&state, |studio| {
        studio.save()?;
        Ok(studio.state())
    })
}

/// Re-read from disk, discarding unsaved changes. This is how an edit made over MCP
/// reaches the canvas.
#[tauri::command]
fn project_reload(app: tauri::AppHandle, state: State<AppState>) -> Result<EditorState> {
    let standard = standard_animations(&app);
    with_studio(&state, |studio| {
        studio.reload(standard.as_deref())?;
        Ok(studio.state())
    })
}

#[tauri::command]
fn project_export(state: State<AppState>, out: Option<String>) -> Result<ExportResult> {
    with_studio(&state, |studio| studio.export(out.map(PathBuf::from)))
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

#[tauri::command]
fn doc_patch(state: State<AppState>, ops: Vec<Op>, label: String) -> Result<PatchResult> {
    with_studio(&state, |studio| studio.patch(label, ops))
}

#[tauri::command]
fn doc_undo(state: State<AppState>) -> Result<EditorState> {
    with_studio(&state, Studio::undo)
}

#[tauri::command]
fn doc_redo(state: State<AppState>) -> Result<EditorState> {
    with_studio(&state, Studio::redo)
}

#[tauri::command]
fn doc_query(
    state: State<AppState>,
    selector: String,
    page: Option<String>,
) -> Result<Vec<String>> {
    with_studio(&state, |studio| studio.query(&selector, page.as_deref()))
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
    with_studio(&state, |studio| {
        studio.snapshot(&page, time, width, node_id.as_deref())
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

#[tauri::command]
fn anim_list(state: State<AppState>) -> Result<Vec<PackageInfo>> {
    with_studio(&state, |studio| Ok(studio.animations()))
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
        studio.bake(&package, &selector, params, &page)
    })
}

#[tauri::command]
fn anim_rebake(
    state: State<AppState>,
    timeline_id: String,
    page: String,
    params: serde_json::Map<String, serde_json::Value>,
) -> Result<md_doc::Timeline> {
    with_studio(&state, |studio| studio.rebake(&timeline_id, &page, params))
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
    // A viewport that is not four numbers is dropped rather than refused: it is a hint
    // for framing a screenshot, and losing it costs the agent nothing it cannot recover.
    let viewport = viewport.and_then(|v| <[f64; 4]>::try_from(v).ok());
    with_studio(&state, |studio| {
        studio.publish_selection(page, &nodes, note, viewport)
    })
}

#[tauri::command]
fn request_queue(
    state: State<AppState>,
    note: String,
    page: String,
    nodes: Vec<String>,
) -> Result<String> {
    with_studio(&state, |studio| studio.queue_request(note, page, &nodes))
}

/// The command line to attach an agent to this project, for the UI to show verbatim.
#[tauri::command]
fn mcp_command(state: State<AppState>) -> Result<String> {
    with_studio(&state, |studio| Ok(studio.mcp_command()))
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
