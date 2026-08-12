//! Watching the project directory.
//!
//! Without this, the architecture's central promise quietly fails. An AI editing over
//! MCP writes the project to disk; the studio, holding its own copy in memory, never
//! notices; and the studio's next save writes that stale copy back over the top. The
//! model's work does not merely go unseen — it is destroyed, and nobody is told.
//!
//! So the studio watches the files it owns and reloads when someone else changes them.
//!
//! The hard part is not the watching, it is telling *our* writes apart from *theirs*.
//! The studio saves after every patch, and a naive watcher would see its own save,
//! reload, and in doing so throw away the undo history for a change it just made. Two
//! things prevent that:
//!
//! * [`WatchGate::mark_self_write`] is called immediately before the studio writes, and
//!   events arriving within [`SELF_WRITE_GRACE`] of that mark are ignored.
//! * Events are debounced, so the burst of writes a single save produces — one file per
//!   page, plus `project.json` — arrives as one event rather than several.

use notify::RecursiveMode;
use notify_debouncer_mini::new_debouncer;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// How long after our own save to keep ignoring filesystem events.
///
/// Generous on purpose. The cost of being too slow is that a change made by an agent in
/// the same half-second as one of ours is missed until the next one; the cost of being
/// too quick is reloading over the user's own edit and losing their undo stack. The
/// second is much worse, and the first practically does not happen — an agent that just
/// wrote is not about to write again instantly.
const SELF_WRITE_GRACE: Duration = Duration::from_millis(1200);

/// How long to wait for a burst of writes to settle before reporting it.
const DEBOUNCE: Duration = Duration::from_millis(350);

/// Event emitted to the frontend when the project changed underneath us.
pub const PROJECT_CHANGED: &str = "project-changed";

/// Shared marker recording when the studio last wrote to the project itself.
///
/// Milliseconds since the epoch, in an atomic so the save path can stamp it without
/// taking the editor lock the watcher thread would otherwise contend on.
#[derive(Debug, Default)]
pub struct WatchGate {
    last_self_write_ms: AtomicU64,
}

impl WatchGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call immediately *before* writing the project to disk.
    pub fn mark_self_write(&self) {
        self.last_self_write_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// Was the most recent write ours?
    fn is_recent_self_write(&self) -> bool {
        let last = self.last_self_write_ms.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        now_ms().saturating_sub(last) < SELF_WRITE_GRACE.as_millis() as u64
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Does this path represent a change to the design itself?
///
/// The project directory holds a good deal that is not the document: the export output,
/// session state the studio itself writes constantly, and the temp files an atomic save
/// creates and immediately renames away. Reloading on any of those would be at best
/// wasted work and at worst an endless loop, since `selection.json` is rewritten every
/// time the selection changes.
fn is_document_change(path: &Path) -> bool {
    let text = path.to_string_lossy();

    if text.contains("/.md/") || text.contains("\\.md\\") {
        return false;
    }
    if text.contains("/dist/") || text.contains("\\dist\\") {
        return false;
    }
    if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
        return false;
    }

    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "project.json" || name == "tokens.json" {
        return true;
    }
    // A page, an animation package, or a queued request.
    matches!(path.extension().and_then(|e| e.to_str()), Some("json"))
}

/// Start watching a project directory. The returned handle must be kept alive; dropping
/// it stops the watcher.
pub fn watch_project(
    app: AppHandle,
    dir: PathBuf,
    gate: Arc<WatchGate>,
) -> notify::Result<Box<dyn std::any::Any + Send>> {
    let watched = dir.clone();

    let mut debouncer = new_debouncer(DEBOUNCE, move |result| {
        let events: Vec<notify_debouncer_mini::DebouncedEvent> = match result {
            Ok(events) => events,
            Err(e) => {
                eprintln!("project watcher error: {e:?}");
                return;
            }
        };

        if !events.iter().any(|e| is_document_change(&e.path)) {
            return;
        }
        if gate.is_recent_self_write() {
            return;
        }

        // The frontend decides what to do with this. It reloads, but only when it has no
        // unsaved changes of its own — see the listener in App.tsx. Emitting rather than
        // reloading here keeps that policy in one place, next to the user interface that
        // has to explain it.
        if let Err(e) = app.emit(PROJECT_CHANGED, watched.to_string_lossy().to_string()) {
            eprintln!("could not report a project change: {e}");
        }
    })?;

    debouncer.watcher().watch(&dir, RecursiveMode::Recursive)?;

    // The debouncer stops when dropped, so it is handed back to be parked in app state.
    Ok(Box::new(debouncer))
}

/// Test hook: how long the grace period lasts.
pub fn self_write_grace() -> Duration {
    SELF_WRITE_GRACE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_writes_are_ignored_for_the_grace_period() {
        let gate = WatchGate::new();
        assert!(!gate.is_recent_self_write(), "a gate that never wrote should not gate");

        gate.mark_self_write();
        assert!(gate.is_recent_self_write());
    }

    #[test]
    fn a_write_long_enough_ago_stops_being_ignored() {
        let gate = WatchGate::new();
        // Stamp a time comfortably outside the window rather than sleeping for it.
        gate.last_self_write_ms
            .store(now_ms() - SELF_WRITE_GRACE.as_millis() as u64 - 100, Ordering::Relaxed);
        assert!(!gate.is_recent_self_write());
    }

    #[test]
    fn session_state_and_build_output_do_not_count_as_document_changes() {
        // `.md/selection.json` is rewritten on every selection change. Reloading on it
        // would be an endless loop.
        assert!(!is_document_change(Path::new("/p/.md/selection.json")));
        assert!(!is_document_change(Path::new("/p/dist/index.html")));
        assert!(!is_document_change(Path::new("/p/pages/index.tmp")));
    }

    #[test]
    fn pages_and_project_metadata_do_count() {
        assert!(is_document_change(Path::new("/p/project.json")));
        assert!(is_document_change(Path::new("/p/tokens.json")));
        assert!(is_document_change(Path::new("/p/pages/index.json")));
        assert!(is_document_change(Path::new("/p/requests/req_1.json")));
        assert!(is_document_change(Path::new(
            "/p/animations/my-fade/manifest.json"
        )));
    }

    #[test]
    fn unrelated_files_are_ignored() {
        assert!(!is_document_change(Path::new("/p/README.md")));
        assert!(!is_document_change(Path::new("/p/assets/photo.png")));
    }

    #[test]
    fn the_grace_period_is_longer_than_the_debounce() {
        // Otherwise a save's own event could arrive after the gate had already reopened,
        // and the studio would reload over the change it had just made.
        assert!(
            self_write_grace() > DEBOUNCE,
            "grace {SELF_WRITE_GRACE:?} must exceed debounce {DEBOUNCE:?}"
        );
    }
}
