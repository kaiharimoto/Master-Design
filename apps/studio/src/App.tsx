import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { BackendUnavailable, hasBackend } from "./ipc";
import { prefersCoarsePointer, shellFor, type ShellKind } from "./input";
import { Shell } from "./Shell";
import { actions, state } from "./store";
import { Updates } from "./Updates";

export function App() {
  const [kind, setKind] = createSignal<ShellKind>("desktop");
  const [coarse, setCoarse] = createSignal(false);
  const [changedOutside, setChangedOutside] = createSignal(false);

  /**
   * Pick up changes made to the project by something other than this window — in
   * practice, an AI editing over MCP.
   *
   * Without this the studio holds a stale copy and its next save writes that copy back
   * over the model's work, destroying it silently. The backend watches the files and
   * emits an event; the policy for what to do about it lives here, next to the interface
   * that has to explain it.
   */
  async function watchForExternalChanges() {
    if (!hasBackend()) return;
    const { listen } = await import("@tauri-apps/api/event");

    return listen("project-changed", () => {
      // The backend already filtered out its own writes. Anything that reaches here is
      // somebody else's, so take it — the alternative is holding a copy we know to be
      // out of date.
      void actions.reload().then(() => {
        setChangedOutside(true);
        window.setTimeout(() => setChangedOutside(false), 5000);
      });
    });
  }

  onMount(() => {
    const measure = () => {
      const isCoarse = prefersCoarsePointer();
      setCoarse(isCoarse);
      setKind(shellFor(window.innerWidth, window.innerHeight, isCoarse));
    };
    measure();

    window.addEventListener("resize", measure);
    // Rotating a phone mid-edit must not lose the layout; orientationchange fires
    // before the resize on some Android WebViews, so both are listened for.
    window.addEventListener("orientationchange", measure);
    window.addEventListener("keydown", onKeyDown);

    void actions.init();

    const unlisten = watchForExternalChanges();

    onCleanup(() => {
      void unlisten.then((stop) => stop?.());
      window.removeEventListener("resize", measure);
      window.removeEventListener("orientationchange", measure);
      window.removeEventListener("keydown", onKeyDown);
    });
  });

  function onKeyDown(e: KeyboardEvent) {
    const target = e.target as HTMLElement | null;
    // Never steal a keystroke from a field the person is typing into.
    if (target && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)) return;

    const mod = e.metaKey || e.ctrlKey;

    if (mod && e.key.toLowerCase() === "z") {
      e.preventDefault();
      if (e.shiftKey) void actions.redo();
      else void actions.undo();
      return;
    }
    if (mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      void actions.save();
      return;
    }
    if (mod && e.shiftKey && e.key.toLowerCase() === "e") {
      e.preventDefault();
      void actions.exportSite();
      return;
    }
    if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault();
      actions.selectAll();
      return;
    }

    switch (e.key) {
      case "Delete":
      case "Backspace":
        e.preventDefault();
        void actions.deleteSelection();
        break;
      case "Escape":
        actions.clearSelection();
        actions.setTool("select");
        break;
      case "v":
      case "V":
        actions.setTool("select");
        break;
      case "p":
      case "P":
        actions.setTool("pen");
        break;
      case "r":
      case "R":
        actions.setTool("rect");
        break;
      case "o":
      case "O":
        actions.setTool("ellipse");
        break;
      case "t":
      case "T":
        actions.setTool("text");
        break;
      case "h":
      case "H":
        actions.setTool("hand");
        break;
      case "1":
        actions.fitToPage();
        break;
      case "2":
        actions.zoomToSelection();
        break;
    }
  }

  return (
    <Show when={hasBackend()} fallback={<NoBackend />}>
      <>
        <Show when={state.editor?.document} fallback={<Welcome />}>
          <Shell kind={kind()} coarse={coarse()} />
        </Show>
        <Updates />
        <Show when={changedOutside()}>
          <div class="toast toast--info" role="status">
            <span>Reloaded — the project changed on disk.</span>
          </div>
        </Show>
        <Show when={state.notice}>
          {(notice) => (
            <div class="toast toast--info" role="status">
              <div class="toast__body">
                <span>{notice().text}</span>
                {/*
                  Warnings belong in front of the person who is about to publish, not in a
                  console they will never open. An export that quietly dropped an
                  animation is worse than one that says it did.
                */}
                <Show when={notice().detail.length > 0}>
                  <ul class="toast__detail">
                    {notice().detail.map((line) => (
                      <li>{line}</li>
                    ))}
                  </ul>
                </Show>
              </div>
              <button onClick={() => actions.dismissNotice()} aria-label="Dismiss">
                ×
              </button>
            </div>
          )}
        </Show>
        <Show when={state.error}>
          {(message) => (
            <div class="toast toast--error" role="alert">
              <span>{message()}</span>
              <button onClick={() => actions.dismissError()} aria-label="Dismiss">
                ×
              </button>
            </div>
          )}
        </Show>
      </>
    </Show>
  );
}

/**
 * Shown in a plain browser tab.
 *
 * There is deliberately no JavaScript stand-in for the backend. A mock would be a
 * second implementation of the patch protocol that quietly disagreed with the real one,
 * which is the exact failure this architecture exists to prevent.
 */
function NoBackend() {
  return (
    <div class="splash">
      <h1>Master Design</h1>
      <p>
        This is the interface running without its backend, so there is nothing to edit.
        The document, the patch protocol and the undo stack all live in the app itself.
      </p>
      <pre>
        <code>pnpm tauri:dev</code>
      </pre>
      <p class="splash__fine">{new BackendUnavailable().message}</p>
    </div>
  );
}

function Welcome() {
  const [path, setPath] = createSignal("");
  const [name, setName] = createSignal("Untitled");

  async function browse() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen === "string") {
      setPath(chosen);
      void actions.open(chosen);
    }
  }

  return (
    <div class="splash">
      <h1>Master Design</h1>
      <p>Design and build graphic animated websites, with an AI model as a co-editor.</p>

      <div class="splash__actions">
        <button class="splash__primary" onClick={() => void browse()}>
          Open a project
        </button>
      </div>

      <div class="splash__new">
        <h2>Or start a new one</h2>
        <label>
          <span>Name</span>
          <input value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        </label>
        <label>
          <span>Folder</span>
          <input
            value={path()}
            placeholder="/path/to/my-site"
            onInput={(e) => setPath(e.currentTarget.value)}
          />
        </label>
        <button
          disabled={!path().trim()}
          onClick={() => void actions.create(path().trim(), name().trim() || "Untitled", 1440, 900)}
        >
          Create
        </button>
      </div>

      <Show when={state.error}>
        {(message) => <p class="splash__error">{message()}</p>}
      </Show>
    </div>
  );
}
