/**
 * The visual bridge, as a piece of interface.
 *
 * Everything else in the app is a design tool. This strip is the part that makes the
 * tool a conversation: the person points at something and says what they want, and that
 * pointing plus that sentence become something a coding model can act on.
 *
 * It works two ways, and which one is available depends on whether an agent is
 * attached:
 *
 * - **Attached** — the note and the selection are published to `.md/selection.json`,
 *   which the MCP server reads through `selection_get`. The model can answer "make this
 *   bounce" without being told what "this" is.
 * - **Not attached** — the note is queued into the project as a request, travels with
 *   it through git, and is picked up later. This is the only path that exists on a
 *   phone, where there is no terminal to run an agent in, and designing around it is
 *   what keeps the mobile app from being a viewer.
 */

import { createResource, createSignal, Show } from "solid-js";
import * as ipc from "./ipc";
import { actions, note, page, setNote, state } from "./store";

export function BridgeBar(props: { compact: boolean }) {
  const [command] = createResource(() => ipc.bridge.mcpCommand().catch(() => null));
  const [queued, setQueued] = createSignal<string | null>(null);
  const [showHelp, setShowHelp] = createSignal(false);

  const hasSelection = () => state.selection.length > 0;

  async function send() {
    const text = note().trim();
    const p = page();
    if (!text || !p) return;

    // Publish for an attached agent, and queue for one that is not. Doing both is
    // deliberate: the person should not have to know which case they are in.
    await actions.publishSelection();
    const id = await ipc.bridge.queueRequest(text, p.slug, [...state.selection]);

    setQueued(id);
    setNote("");
    window.setTimeout(() => setQueued(null), 4000);
  }

  return (
    <div class="bridge" classList={{ "bridge--compact": props.compact }}>
      <input
        class="bridge__note"
        type="text"
        value={note()}
        placeholder={
          hasSelection()
            ? `Ask for a change to ${state.selection.length === 1 ? "this" : "these"}…`
            : "Ask for a change…"
        }
        aria-label="Describe a change for the AI"
        onInput={(e) => setNote(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void send();
        }}
      />

      <Show when={hasSelection()}>
        <span class="bridge__context" title="What the note is about">
          {state.selection.length} selected
        </span>
      </Show>

      <button class="bridge__send" disabled={!note().trim()} onClick={() => void send()}>
        Send
      </button>

      <button
        class="bridge__help"
        aria-label="How to attach an AI model"
        aria-expanded={showHelp()}
        onClick={() => setShowHelp(!showHelp())}
      >
        ?
      </button>

      <Show when={queued()}>
        <span class="bridge__toast" role="status">
          Queued. An attached model sees it now; otherwise it waits in the project.
        </span>
      </Show>

      <Show when={showHelp()}>
        <div class="bridge__panel">
          <h3>Attaching a model</h3>
          <p>
            Master Design speaks the Model Context Protocol. Point any MCP client — Claude
            Code, for instance — at this project and it can read the design, change it, and
            look at the result.
          </p>
          <Show when={command()} fallback={<p>Open a project to see the command.</p>}>
            {(cmd) => (
              <pre class="bridge__command">
                <code>{cmd()}</code>
              </pre>
            )}
          </Show>
          <p class="bridge__fine">
            Whatever it changes arrives through the same operations your own edits do, so
            you can undo it with ⌘Z like anything else.
          </p>
        </div>
      </Show>
    </div>
  );
}
