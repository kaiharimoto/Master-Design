/**
 * The inspector.
 *
 * Two things here are not decoration. **Roles** are how animations and AI instructions
 * address nodes, so they get first-class editing rather than living in a metadata
 * drawer. **Accessibility** is filled in here or not at all: it is impossible to
 * retrofit heading structure onto flat vector output after the fact, and an exported
 * page that no screen reader can navigate is not finished.
 *
 * Every field commits one operation on change, so each adjustment is one undo step and
 * the backend validates before anything lands.
 */

import { createMemo, For, Show } from "solid-js";
import * as m from "../math";
import { actions, selectedNodes, state } from "../store";
import type { Node, Paint } from "../types";

export function Inspector() {
  const nodes = selectedNodes;
  const first = () => nodes()[0] ?? null;

  /** Show a value only when every selected node agrees on it. */
  const shared = <T,>(read: (n: Node) => T): T | null => {
    const all = nodes().map(read);
    if (all.length === 0) return null;
    return all.every((v) => JSON.stringify(v) === JSON.stringify(all[0])) ? all[0] : null;
  };

  return (
    <div class="panel panel--inspector">
      <header class="panel__header">
        <h2>Properties</h2>
        <Show when={nodes().length > 1}>
          <span class="panel__badge">{nodes().length} layers</span>
        </Show>
      </header>

      <div class="panel__body">
        <Show
          when={first()}
          fallback={<p class="panel__empty">Select something to edit its properties.</p>}
        >
          {(node) => (
            <>
              <Section title="Layer">
                <Field label="Name">
                  <input
                    type="text"
                    value={shared((n) => n.name ?? "") ?? ""}
                    placeholder={nodes().length > 1 ? "Mixed" : node().type}
                    onChange={(e) =>
                      void actions.setProperty("name", e.currentTarget.value, "Rename layer")
                    }
                  />
                </Field>
                <Field label="Opacity">
                  <Slider
                    value={shared((n) => n.opacity ?? 1) ?? 1}
                    min={0}
                    max={1}
                    step={0.01}
                    onCommit={(v) => void actions.setProperty("opacity", v, "Change opacity")}
                  />
                </Field>
              </Section>

              <Transform node={node()} />
              <Geometry node={node()} />
              <Show when={node().type === "text"}>
                <Typography node={node()} />
              </Show>
              <Fills node={node()} />
              <Strokes node={node()} />
              <Roles node={node()} />
              <Accessibility node={node()} />
            </>
          )}
        </Show>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

function Transform(props: { node: Node }) {
  const d = createMemo(() => m.decompose(props.node.transform ?? m.IDENTITY));

  const write = (next: Partial<m.Decomposed>, label: string) => {
    const current = d();
    const merged = { ...current, ...next };
    const matrix = m.then(
      m.then(m.scale(merged.scaleX, merged.scaleY), m.rotate(merged.rotation)),
      m.translate(merged.x, merged.y),
    );
    void actions.setProperty("transform", matrix, label);
  };

  return (
    <Section title="Position">
      <div class="grid grid--2">
        <Field label="X">
          <Num value={d().x} onCommit={(v) => write({ x: v }, "Move")} />
        </Field>
        <Field label="Y">
          <Num value={d().y} onCommit={(v) => write({ y: v }, "Move")} />
        </Field>
        <Field label="Rotation">
          <Num
            value={m.degrees(d().rotation)}
            suffix="°"
            onCommit={(v) => write({ rotation: m.radians(v) }, "Rotate")}
          />
        </Field>
        <Field label="Scale">
          <Num
            value={d().scaleX}
            step={0.01}
            onCommit={(v) => write({ scaleX: v, scaleY: v }, "Scale")}
          />
        </Field>
      </div>
    </Section>
  );
}

function Geometry(props: { node: Node }) {
  const sized = () => ["frame", "rect", "ellipse", "image"].includes(props.node.type);
  const rounded = () => ["frame", "rect"].includes(props.node.type);

  return (
    <Show when={sized()}>
      <Section title="Size">
        <div class="grid grid--2">
          <Field label="Width">
            <Num
              value={props.node.width ?? 0}
              min={0}
              onCommit={(v) => void actions.setProperty("width", v, "Resize")}
            />
          </Field>
          <Field label="Height">
            <Num
              value={props.node.height ?? 0}
              min={0}
              onCommit={(v) => void actions.setProperty("height", v, "Resize")}
            />
          </Field>
        </div>
        <Show when={rounded()}>
          <Field label="Corner radius">
            <Num
              value={props.node.cornerRadius?.[0] ?? 0}
              min={0}
              onCommit={(v) =>
                void actions.setProperty("cornerRadius", [v, v, v, v], "Round corners")
              }
            />
          </Field>
        </Show>
      </Section>
    </Show>
  );
}

function Typography(props: { node: Node }) {
  return (
    <Section title="Type">
      <Field label="Content">
        <textarea
          rows="3"
          value={props.node.content ?? ""}
          onChange={(e) =>
            void actions.setProperty("content", e.currentTarget.value, "Edit text")
          }
        />
      </Field>
      <div class="grid grid--2">
        <Field label="Font">
          <input
            type="text"
            value={props.node.fontFamily ?? "Inter"}
            onChange={(e) =>
              void actions.setProperty("fontFamily", e.currentTarget.value, "Change font")
            }
          />
        </Field>
        <Field label="Size">
          <Num
            value={props.node.fontSize ?? 16}
            min={1}
            onCommit={(v) => void actions.setProperty("fontSize", v, "Change size")}
          />
        </Field>
        <Field label="Weight">
          <select
            value={String(props.node.fontWeight ?? 400)}
            onChange={(e) =>
              void actions.setProperty("fontWeight", Number(e.currentTarget.value), "Change weight")
            }
          >
            <For each={[100, 200, 300, 400, 500, 600, 700, 800, 900]}>
              {(w) => <option value={String(w)}>{w}</option>}
            </For>
          </select>
        </Field>
        <Field label="Line height">
          <Num
            value={props.node.lineHeight ?? 1.4}
            step={0.05}
            min={0.5}
            onCommit={(v) => void actions.setProperty("lineHeight", v, "Change line height")}
          />
        </Field>
      </div>
      <Field label="Alignment">
        <div class="segmented" role="group" aria-label="Text alignment">
          <For each={["left", "center", "right", "justify"] as const}>
            {(align) => (
              <button
                classList={{ "segmented__on": (props.node.align ?? "left") === align }}
                onClick={() => void actions.setProperty("align", align, "Align text")}
              >
                {align[0].toUpperCase()}
              </button>
            )}
          </For>
        </div>
      </Field>
    </Section>
  );
}

function Fills(props: { node: Node }) {
  const fills = () => props.node.fills ?? [];

  const addFill = () => {
    void actions.setProperty("fills", [...fills(), { type: "solid", color: "#7c3aed" }], "Add fill");
  };

  return (
    <Section
      title="Fill"
      action={
        <button class="mini" onClick={addFill} aria-label="Add fill">
          +
        </button>
      }
    >
      <Show when={fills().length === 0}>
        <p class="panel__hint">No fill.</p>
      </Show>
      <For each={fills()}>
        {(paint, i) => (
          <PaintRow
            paint={paint}
            onColor={(hex) =>
              void actions.setProperty(`fills.${i()}.color`, hex, "Change fill")
            }
            onRemove={() =>
              void actions.setProperty(
                "fills",
                fills().filter((_, j) => j !== i()),
                "Remove fill",
              )
            }
          />
        )}
      </For>
    </Section>
  );
}

function Strokes(props: { node: Node }) {
  const strokes = () => props.node.strokes ?? [];

  return (
    <Section
      title="Stroke"
      action={
        <button
          class="mini"
          aria-label="Add stroke"
          onClick={() =>
            void actions.setProperty(
              "strokes",
              [...strokes(), { paint: { type: "solid", color: "#111111" }, width: 2 }],
              "Add stroke",
            )
          }
        >
          +
        </button>
      }
    >
      <Show when={strokes().length === 0}>
        <p class="panel__hint">No stroke.</p>
      </Show>
      <For each={strokes()}>
        {(stroke, i) => (
          <div class="stroke-row">
            <PaintRow
              paint={stroke.paint}
              onColor={(hex) =>
                void actions.setProperty(`strokes.${i()}.paint.color`, hex, "Change stroke")
              }
              onRemove={() =>
                void actions.setProperty(
                  "strokes",
                  strokes().filter((_, j) => j !== i()),
                  "Remove stroke",
                )
              }
            />
            <Num
              value={stroke.width}
              min={0}
              step={0.5}
              onCommit={(v) =>
                void actions.setProperty(`strokes.${i()}.width`, v, "Change stroke width")
              }
            />
          </div>
        )}
      </For>
    </Section>
  );
}

function Roles(props: { node: Node }) {
  const roles = () => props.node.roles ?? [];

  const setRoles = (next: string[]) => {
    void actions.setProperty("roles", next, "Change roles");
  };

  return (
    <Section title="Roles">
      <p class="panel__hint">
        Semantic tags. Animations and AI instructions address these — <code>@card</code> applies
        to every card, however many there turn out to be.
      </p>
      <div class="tags">
        <For each={roles()}>
          {(role, i) => (
            <span class="tag">
              @{role}
              <button
                aria-label={`Remove role ${role}`}
                onClick={() => setRoles(roles().filter((_, j) => j !== i()))}
              >
                ×
              </button>
            </span>
          )}
        </For>
      </div>
      <input
        type="text"
        placeholder="Add a role, e.g. card"
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          const value = e.currentTarget.value.trim().replace(/^@/, "");
          if (!value || roles().includes(value)) return;
          setRoles([...roles(), value]);
          e.currentTarget.value = "";
        }}
      />
    </Section>
  );
}

function Accessibility(props: { node: Node }) {
  const a11y = () => props.node.a11y ?? {};

  const patch = (next: Record<string, unknown>) => {
    void actions.setProperty("a11y", { ...a11y(), ...next }, "Change accessibility");
  };

  return (
    <Section title="Accessibility">
      <p class="panel__hint">
        Exported as a hidden semantic document alongside the artwork, because vector
        shapes carry no meaning on their own.
      </p>
      <Show when={props.node.type === "text"}>
        <Field label="Heading level">
          <select
            value={String(a11y().headingLevel ?? 0)}
            onChange={(e) => {
              const level = Number(e.currentTarget.value);
              patch({ headingLevel: level === 0 ? undefined : level });
            }}
          >
            <option value="0">Body text</option>
            <For each={[1, 2, 3, 4, 5, 6]}>
              {(l) => <option value={String(l)}>Heading {l}</option>}
            </For>
          </select>
        </Field>
      </Show>
      <Field label="Landmark">
        <select
          value={a11y().role ?? ""}
          onChange={(e) => patch({ role: e.currentTarget.value || undefined })}
        >
          <option value="">None</option>
          <For each={["banner", "navigation", "main", "contentinfo", "complementary", "region"]}>
            {(role) => <option value={role}>{role}</option>}
          </For>
        </select>
      </Field>
      <Field label="Label or alt text">
        <input
          type="text"
          value={a11y().label ?? a11y().alt ?? ""}
          onChange={(e) => patch({ label: e.currentTarget.value || undefined })}
        />
      </Field>
      <label class="checkbox">
        <input
          type="checkbox"
          checked={a11y().hidden ?? false}
          onChange={(e) => patch({ hidden: e.currentTarget.checked || undefined })}
        />
        Decorative — hide from screen readers
      </label>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Small controls
// ---------------------------------------------------------------------------

export function Section(props: { title: string; action?: unknown; children: unknown }) {
  return (
    <section class="section">
      <div class="section__head">
        <h3>{props.title}</h3>
        {props.action as never}
      </div>
      <div class="section__body">{props.children as never}</div>
    </section>
  );
}

export function Field(props: { label: string; children: unknown }) {
  return (
    <label class="field">
      <span class="field__label">{props.label}</span>
      {props.children as never}
    </label>
  );
}

/**
 * A number input that commits on blur and on Enter, not on every keystroke.
 *
 * Committing per keystroke would put an undo step between every digit, and "12" would
 * briefly be "1" — which for a width means the layer visibly collapses while you type.
 */
export function Num(props: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  suffix?: string;
  onCommit: (value: number) => void;
}) {
  const commit = (raw: string) => {
    const parsed = Number(raw);
    if (!Number.isFinite(parsed)) return;
    const clamped = m.clamp(parsed, props.min ?? -Infinity, props.max ?? Infinity);
    if (clamped !== props.value) props.onCommit(clamped);
  };

  return (
    <span class="num">
      <input
        type="number"
        value={m.fmt(props.value)}
        step={props.step ?? 1}
        min={props.min}
        max={props.max}
        onChange={(e) => commit(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit(e.currentTarget.value);
        }}
      />
      <Show when={props.suffix}>
        <span class="num__suffix">{props.suffix}</span>
      </Show>
    </span>
  );
}

export function Slider(props: {
  value: number;
  min: number;
  max: number;
  step: number;
  onCommit: (value: number) => void;
}) {
  return (
    <span class="slider">
      <input
        type="range"
        value={props.value}
        min={props.min}
        max={props.max}
        step={props.step}
        // `change` rather than `input`: dragging a slider should be one undo step, not
        // one per pixel of travel.
        onChange={(e) => props.onCommit(Number(e.currentTarget.value))}
      />
      <span class="slider__value">{m.fmt(props.value)}</span>
    </span>
  );
}

function PaintRow(props: {
  paint: Paint;
  onColor: (hex: string) => void;
  onRemove: () => void;
}) {
  const isSolid = () => props.paint.type === "solid";
  const hex = () => (props.paint.type === "solid" ? props.paint.color.slice(0, 7) : "#000000");

  return (
    <div class="paint-row">
      <Show
        when={isSolid()}
        fallback={<span class="paint-row__gradient">{props.paint.type}</span>}
      >
        <input
          type="color"
          value={hex()}
          aria-label="Colour"
          onChange={(e) => props.onColor(e.currentTarget.value)}
        />
        <input
          type="text"
          class="paint-row__hex"
          value={hex()}
          onChange={(e) => props.onColor(e.currentTarget.value)}
        />
      </Show>
      <button class="mini" aria-label="Remove" onClick={props.onRemove}>
        ×
      </button>
    </div>
  );
}

export function selectionSummary(): string {
  const count = state.selection.length;
  if (count === 0) return "Nothing selected";
  if (count === 1) return selectedNodes()[0]?.name || selectedNodes()[0]?.type || "1 layer";
  return `${count} layers`;
}
