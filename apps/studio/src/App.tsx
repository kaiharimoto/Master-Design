import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { BackendUnavailable, hasBackend } from "./ipc";
import { prefersCoarsePointer, shellFor, type ShellKind } from "./input";
import { Shell } from "./Shell";
import { actions, state } from "./store";
import { Updates } from "./Updates";

export function App() {
  const [kind, setKind] = createSignal<ShellKind>("desktop");
  const [coarse, setCoarse] = createSignal(false);

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

    onCleanup(() => {
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
