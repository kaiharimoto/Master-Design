/**
 * The adaptive shell.
 *
 * One view-model, three chromes. The layout is chosen from the shape of the window and
 * whether the pointer can hover — not from a platform check — because a Windows tablet
 * held in portrait wants the touch layout and a phone in a landscape dock does not.
 *
 * - **Desktop** — rails either side, canvas in the middle, everything visible at once.
 * - **Tablet** — same bones, larger targets, rails that collapse to give the artwork
 *   the screen back.
 * - **Phone** — canvas first. Tools sit within thumb reach at the bottom; panels are a
 *   sheet that comes up over the canvas rather than stealing width from it.
 */

import { createSignal, Show } from "solid-js";
import { Canvas } from "./canvas/Canvas";
import type { ShellKind } from "./input";
import { actions, state, tool, type ToolId } from "./store";
import { Animations } from "./panels/Animations";
import { Inspector, selectionSummary } from "./panels/Inspector";
import { Breadcrumb, Layers, Pages } from "./panels/Layers";
import { BridgeBar } from "./Bridge";

const TOOLS: { id: ToolId; label: string; glyph: string; key: string }[] = [
  { id: "select", label: "Select", glyph: "⌖", key: "V" },
  { id: "pen", label: "Pen", glyph: "✎", key: "P" },
  { id: "rect", label: "Rectangle", glyph: "▭", key: "R" },
  { id: "ellipse", label: "Ellipse", glyph: "◯", key: "O" },
  { id: "text", label: "Text", glyph: "T", key: "T" },
  { id: "hand", label: "Pan", glyph: "✋", key: "H" },
];

export function Shell(props: { kind: ShellKind; coarse: boolean }) {
  const [sheet, setSheet] = createSignal<"none" | "layers" | "properties" | "motion">("none");

  const isPhone = () => props.kind === "phone";

  return (
    <div class="shell" data-shell={props.kind}>
      <TopBar kind={props.kind} />

      <div class="shell__body">
        <Show when={!isPhone()}>
          <aside class="rail rail--left" aria-label="Layers">
            <Pages />
            <Layers />
          </aside>
        </Show>

        <main class="shell__canvas">
          <Canvas coarse={props.coarse} />
          <Show when={!isPhone()}>
            <Breadcrumb />
          </Show>
          <Toolbar kind={props.kind} />
        </main>

        <Show when={!isPhone()}>
          <aside class="rail rail--right" aria-label="Properties and motion">
            <Inspector />
            <Animations />
          </aside>
        </Show>
      </div>

      <Show when={isPhone()}>
        <>
          <nav class="tabbar" aria-label="Panels">
            <button
              classList={{ "tabbar__on": sheet() === "layers" }}
              onClick={() => setSheet(sheet() === "layers" ? "none" : "layers")}
            >
              Layers
            </button>
            <button
              classList={{ "tabbar__on": sheet() === "properties" }}
              onClick={() => setSheet(sheet() === "properties" ? "none" : "properties")}
            >
              {selectionSummary()}
            </button>
            <button
              classList={{ "tabbar__on": sheet() === "motion" }}
              onClick={() => setSheet(sheet() === "motion" ? "none" : "motion")}
            >
              Motion
            </button>
          </nav>

          <Show when={sheet() !== "none"}>
            <div class="sheet" role="dialog" aria-modal="false">
              <button
                class="sheet__grip"
                aria-label="Close panel"
                onClick={() => setSheet("none")}
              />
              <div class="sheet__body">
                <Show when={sheet() === "layers"}>
                  <Pages />
                  <Layers />
                </Show>
                <Show when={sheet() === "properties"}>
                  <Inspector />
                </Show>
                <Show when={sheet() === "motion"}>
                  <Animations />
                </Show>
              </div>
            </div>
          </Show>
        </>
      </Show>

      <BridgeBar compact={isPhone()} />
    </div>
  );
}

function TopBar(props: { kind: ShellKind }) {
  const editor = () => state.editor;

  return (
    <header class="topbar">
      <span class="topbar__title">
        {editor()?.document?.meta?.name ?? "Master Design"}
        <Show when={editor()?.dirty}>
          <span class="topbar__dot" title="Unsaved changes" aria-label="Unsaved changes" />
        </Show>
      </span>

      <div class="topbar__actions">
        <button
          disabled={!editor()?.canUndo}
          title={editor()?.undoLabel ? `Undo ${editor()!.undoLabel}` : "Undo"}
          onClick={() => void actions.undo()}
        >
          ↶
        </button>
        <button
          disabled={!editor()?.canRedo}
          title={editor()?.redoLabel ? `Redo ${editor()!.redoLabel}` : "Redo"}
          onClick={() => void actions.redo()}
        >
          ↷
        </button>

        <Show when={props.kind !== "phone"}>
          <div class="topbar__group" role="group" aria-label="Combine shapes">
            <button
              disabled={state.selection.length < 2}
              title="Union"
              onClick={() => void actions.boolean("union")}
            >
              ∪
            </button>
            <button
              disabled={state.selection.length < 2}
              title="Subtract"
              onClick={() => void actions.boolean("subtract")}
            >
              ∖
            </button>
            <button
              disabled={state.selection.length < 2}
              title="Intersect"
              onClick={() => void actions.boolean("intersect")}
            >
              ∩
            </button>
            <button
              disabled={state.selection.length < 2}
              title="Exclude"
              onClick={() => void actions.boolean("exclude")}
            >
              ⊕
            </button>
          </div>
        </Show>

        <button title="Fit to page" onClick={() => actions.fitToPage()}>
          ⤢
        </button>
        <button onClick={() => void actions.save()}>Save</button>
        <button
          class="topbar__go"
          title="Build the static site into the project's dist folder (Ctrl+Shift+E)"
          onClick={() => void actions.exportSite()}
        >
          Export
        </button>
      </div>
    </header>
  );
}

/**
 * The tool palette.
 *
 * Floats over the canvas at the bottom on touch, where the thumb is, and pins to the
 * left edge on desktop, where the muscle memory is.
 */
function Toolbar(props: { kind: ShellKind }) {
  return (
    <div class="toolbar" data-placement={props.kind === "desktop" ? "left" : "bottom"}>
      {TOOLS.map((t) => (
        <button
          class="toolbar__tool"
          classList={{ "toolbar__tool--on": tool() === t.id }}
          title={`${t.label} (${t.key})`}
          aria-label={t.label}
          aria-pressed={tool() === t.id}
          onClick={() => actions.setTool(t.id)}
        >
          <span aria-hidden="true">{t.glyph}</span>
        </button>
      ))}
    </div>
  );
}

export { TOOLS };
