/**
 * Editor state.
 *
 * The document itself is a mirror of what the backend holds; everything else here —
 * selection, viewport, active tool, in-flight drag — is genuinely the frontend's, and
 * none of it belongs in the saved file.
 *
 * The one subtlety worth understanding is how drags work. Sending an operation per
 * pointer move would mean an IPC round trip per frame and an undo stack with two hundred
 * entries for one gesture. Instead a drag writes to `live`, a transient overlay the
 * renderer composes on top of the document, and commits exactly one operation when the
 * pointer lifts. So the canvas stays at pointer speed, and the undo stack reads the way
 * a person remembers working.
 */

import { batch, createMemo, createSignal } from "solid-js";
import { createStore, produce } from "solid-js/store";
import * as ipc from "./ipc";
import * as m from "./math";
import type { Matrix, Node, NodeId, Op, Page, EditorState } from "./types";

export type ToolId =
  | "select"
  | "pen"
  | "rect"
  | "ellipse"
  | "text"
  | "hand";

export interface Viewport {
  /** Document coordinate at the top-left of the visible area. */
  x: number;
  y: number;
  /** Pixels per document unit. */
  scale: number;
}

interface State {
  editor: EditorState | null;
  loading: boolean;
  error: string | null;
  pageIndex: number;
  selection: NodeId[];
  /** Transforms applied on top of the document mid-gesture, keyed by node. */
  live: Record<NodeId, Matrix>;
  /** Timeline being scrubbed, and where. */
  playhead: { timelineId: string | null; time: number; playing: boolean };
  /**
   * Something worth telling the person that is not a failure — an export that finished,
   * with whatever the exporter had to say about it.
   *
   * Separate from `error` rather than a severity field on one slot, because the two have
   * different lifetimes: an error stays until it is dismissed, a notice with warnings in
   * it should too, and a notice with nothing to report can fade.
   */
  notice: { text: string; detail: string[] } | null;
}

const [state, setState] = createStore<State>({
  editor: null,
  loading: false,
  error: null,
  pageIndex: 0,
  selection: [],
  live: {},
  playhead: { timelineId: null, time: 0, playing: false },
  notice: null,
});

const [viewport, setViewport] = createSignal<Viewport>({ x: 0, y: 0, scale: 1 });
const [tool, setToolSignal] = createSignal<ToolId>("select");
const [note, setNote] = createSignal("");

export { state, viewport, tool, note, setNote, setViewport };

// ---------------------------------------------------------------------------
// Derived views
// ---------------------------------------------------------------------------

export const document_ = createMemo(() => state.editor?.document ?? null);

export const page = createMemo<Page | null>(() => {
  const doc = document_();
  if (!doc) return null;
  return doc.pages[Math.min(state.pageIndex, doc.pages.length - 1)] ?? null;
});

/** Flat index of the current page: node, its parent, and its accumulated transform. */
export const index = createMemo(() => {
  const p = page();
  const map = new Map<NodeId, { node: Node; parent: NodeId | null; world: Matrix; depth: number }>();
  if (!p) return map;

  const walk = (node: Node, parent: NodeId | null, parentWorld: Matrix, depth: number) => {
    const world = m.then(node.transform ?? m.IDENTITY, parentWorld);
    map.set(node.id, { node, parent, world, depth });
    for (const child of node.children ?? []) walk(child, node.id, world, depth + 1);
  };
  walk(p.root, null, m.IDENTITY, 0);
  return map;
});

export const selectedNodes = createMemo(() => {
  const idx = index();
  return state.selection.map((id) => idx.get(id)?.node).filter((n): n is Node => !!n);
});

/** Bounding box of the selection, in page coordinates. */
export const selectionBounds = createMemo<m.Rect | null>(() => {
  const idx = index();
  let acc: m.Rect | null = null;
  for (const id of state.selection) {
    const entry = idx.get(id);
    if (!entry) continue;
    const local = localBounds(entry.node);
    if (!local) continue;
    const world = m.transformRect(withLive(entry.world, id), local);
    acc = acc ? m.rectUnion(acc, world) : world;
  }
  return acc;
});

/**
 * A node's untransformed extent.
 *
 * Mirrors `md_doc::Node::local_bounds`, including the kinds whose size the document does
 * not state outright: everything with a box gets it from the box, a group is the union of
 * its children, and a path is measured. A `null` here is what stops a node being
 * selectable, resizable or catchable by the marquee, so the only kinds that return one
 * are the ones with genuinely nothing to measure yet.
 */
export function localBounds(node: Node): m.Rect | null {
  switch (node.type) {
    case "frame":
    case "rect":
    case "ellipse":
    case "image":
      return { x: 0, y: 0, w: node.width ?? 0, h: node.height ?? 0 };
    case "text": {
      const size = node.fontSize ?? 16;
      const lines = (node.content ?? "").split("\n");
      const longest = Math.max(...lines.map((l) => l.length), 0);
      return {
        x: 0,
        y: 0,
        // An estimate. The real measurement comes from the rendered element via
        // getBBox once it is on screen; this is what the inspector shows before then.
        w: node.width ?? longest * size * 0.55,
        h: node.height ?? lines.length * size * (node.lineHeight ?? 1.4),
      };
    }
    case "group": {
      // A group has no extent of its own; it is exactly what it contains, each child
      // measured in the group's own space. Without this a group could never show a
      // handle, be resized, or be caught by the marquee.
      let acc: m.Rect | null = null;
      for (const child of node.children ?? []) {
        const local = localBounds(child);
        if (!local) continue;
        const inParent = child.transform ? m.transformRect(child.transform, local) : local;
        acc = acc ? m.rectUnion(acc, inParent) : inParent;
      }
      return acc;
    }
    case "path": {
      // The browser has already computed the outline's extent for the element on
      // screen; asking it beats carrying a second path-measuring implementation that
      // would have to agree with `md-geom`. Before the element mounts there is nothing
      // to measure, and the memo re-runs once it does.
      const el = document.querySelector(`[data-node-id="${node.id}"] path`);
      if (!(el instanceof SVGGraphicsElement)) return null;
      const box = el.getBBox();
      return { x: box.x, y: box.y, w: box.width, h: box.height };
    }
  }
}

function withLive(world: Matrix, id: NodeId): Matrix {
  const live = state.live[id];
  return live ? m.then(live, world) : world;
}

export function worldOf(id: NodeId): Matrix {
  const entry = index().get(id);
  return entry ? withLive(entry.world, id) : m.IDENTITY;
}

export function isSelected(id: NodeId): boolean {
  return state.selection.includes(id);
}

// ---------------------------------------------------------------------------
// Coordinate conversion
// ---------------------------------------------------------------------------

export function screenToDoc(sx: number, sy: number): [number, number] {
  const v = viewport();
  return [v.x + sx / v.scale, v.y + sy / v.scale];
}

export function docToScreen(dx: number, dy: number): [number, number] {
  const v = viewport();
  return [(dx - v.x) * v.scale, (dy - v.y) * v.scale];
}

// ---------------------------------------------------------------------------
// Project lifecycle
// ---------------------------------------------------------------------------

async function run<T>(work: () => Promise<T>): Promise<T | null> {
  setState("loading", true);
  setState("error", null);
  try {
    return await work();
  } catch (e) {
    setState("error", e instanceof Error ? e.message : String(e));
    return null;
  } finally {
    setState("loading", false);
  }
}

function adopt(editor: EditorState) {
  batch(() => {
    setState("editor", editor);
    setState("live", {});
    // A node the backend removed must not linger in the selection, or the inspector
    // would be editing something that no longer exists.
    const ids = new Set<NodeId>();
    const collect = (n: Node) => {
      ids.add(n.id);
      for (const c of n.children ?? []) collect(c);
    };
    for (const p of editor.document.pages) collect(p.root);
    setState("selection", (sel) => sel.filter((id) => ids.has(id)));
    if (state.pageIndex >= editor.document.pages.length) setState("pageIndex", 0);
  });
}

export const actions = {
  async init() {
    const editor = await run(() => ipc.project.state());
    if (editor) {
      adopt(editor);
      if (editor.document) actions.fitToPage();
    }
  },

  async open(path: string) {
    const editor = await run(() => ipc.project.open(path));
    if (editor) {
      adopt(editor);
      actions.fitToPage();
    }
  },

  async create(path: string, name: string, width: number, height: number) {
    const editor = await run(() => ipc.project.create(path, name, width, height));
    if (editor) {
      adopt(editor);
      actions.fitToPage();
    }
  },

  async save() {
    const editor = await run(() => ipc.project.save());
    if (editor) adopt(editor);
  },

  /**
   * Build the static site.
   *
   * The output is the actual deliverable — HTML, CSS, SVG and the animation runtime, no
   * framework and no build step — so this is the moment a design becomes a website. It
   * writes to `dist/` inside the project unless told otherwise.
   *
   * Warnings are shown rather than logged. They are the exporter saying "I could not
   * compile this animation property" or "I approximated that stroke alignment", which is
   * precisely what someone about to publish needs to know.
   */
  async exportSite(out?: string) {
    const result = await run(() => ipc.project.export(out));
    if (!result) return;

    const kb = Math.max(1, Math.round(result.bytes / 1024));
    const pages = result.files.filter((f) => f.endsWith(".html")).length;
    setState("notice", {
      text: `Exported ${pages} page${pages === 1 ? "" : "s"} — ${kb} kB`,
      detail: result.warnings,
    });
    return result;
  },

  /** Pick up changes made outside the app — most often by an AI over MCP. */
  async reload() {
    const editor = await run(() => ipc.project.reload());
    if (editor) adopt(editor);
  },

  // -------------------------------------------------------------------------
  // Editing
  // -------------------------------------------------------------------------

  async patch(ops: Op[], label: string) {
    if (ops.length === 0) return;
    const result = await run(() => ipc.edit.patch(ops, label));
    if (result) {
      adopt(result.state);
      for (const w of result.report.warnings) console.warn("[patch]", w);
    }
  },

  async undo() {
    const editor = await run(() => ipc.edit.undo());
    if (editor) adopt(editor);
  },

  async redo() {
    const editor = await run(() => ipc.edit.redo());
    if (editor) adopt(editor);
  },

  async deleteSelection() {
    const ids = [...state.selection];
    if (ids.length === 0) return;
    await actions.patch(
      ids.map((id) => ({ op: "node.delete", id }) as Op),
      ids.length === 1 ? "Delete layer" : `Delete ${ids.length} layers`,
    );
    setState("selection", []);
  },

  /** Set one property on every selected node, as a single undo step. */
  async setProperty(path: string, value: unknown, label?: string) {
    const ops: Op[] = state.selection.map((id) => ({ op: "node.update", id, path, value }));
    await actions.patch(ops, label ?? `Change ${path}`);
  },

  async boolean(mode: "union" | "subtract" | "intersect" | "exclude") {
    if (state.selection.length < 2) {
      setState("error", "Select two or more shapes to combine them.");
      return;
    }
    await actions.patch(
      [{ op: "path.boolean", ids: [...state.selection], mode }],
      `${mode[0].toUpperCase()}${mode.slice(1)}`,
    );
  },

  async reorder(id: NodeId, parent: NodeId, indexInParent: number) {
    await actions.patch([{ op: "node.move", id, parent, index: indexInParent }], "Reorder layer");
  },

  // -------------------------------------------------------------------------
  // Selection
  // -------------------------------------------------------------------------

  select(ids: NodeId[]) {
    setState("selection", ids);
    void actions.publishSelection();
  },

  toggleSelect(id: NodeId) {
    setState(
      "selection",
      produce((sel: NodeId[]) => {
        const at = sel.indexOf(id);
        if (at >= 0) sel.splice(at, 1);
        else sel.push(id);
      }),
    );
    void actions.publishSelection();
  },

  clearSelection() {
    setState("selection", []);
    void actions.publishSelection();
  },

  selectAll() {
    const p = page();
    if (!p) return;
    actions.select((p.root.children ?? []).map((c) => c.id));
  },

  /**
   * Tell any attached agent what the person is looking at.
   *
   * This is the live half of the visual bridge: it turns "make this bounce" into
   * something answerable without the person having to name a node.
   */
  async publishSelection() {
    const p = page();
    if (!p || !state.editor?.projectPath) return;
    const v = viewport();
    const size = canvasSize();
    try {
      await ipc.bridge.publishSelection(
        p.slug,
        [...state.selection],
        note(),
        [v.x, v.y, size[0] / v.scale, size[1] / v.scale],
      );
    } catch {
      // Best effort. A failure here means an agent gets less context, not that the
      // person's edit failed, so it must never surface as an error.
    }
  },

  // -------------------------------------------------------------------------
  // Live gestures
  // -------------------------------------------------------------------------

  setLive(id: NodeId, matrix: Matrix) {
    setState("live", id, matrix);
  },

  clearLive() {
    setState("live", {});
  },

  /** Fold the in-flight transform into the document as one operation per node. */
  async commitLive(label: string) {
    const idx = index();
    const ops: Op[] = [];
    for (const [id, live] of Object.entries(state.live)) {
      const entry = idx.get(id);
      if (!entry || m.isIdentity(live)) continue;
      ops.push({
        op: "node.update",
        id,
        path: "transform",
        value: m.then(entry.node.transform ?? m.IDENTITY, live),
      });
    }
    setState("live", {});
    if (ops.length) await actions.patch(ops, label);
  },

  // -------------------------------------------------------------------------
  // Viewport
  // -------------------------------------------------------------------------

  setTool(id: ToolId) {
    setToolSignal(id);
  },

  zoomBy(factor: number, atScreenX: number, atScreenY: number) {
    setViewport((v) => {
      // Zoom limits: below 2% a page is a speck, above 6400% a device pixel is a
      // hundredth of a unit and nothing snaps usefully.
      const scale = m.clamp(v.scale * factor, 0.02, 64);
      const applied = scale / v.scale;
      return {
        scale,
        // Keep the point under the cursor or the pinch centre exactly where it is.
        x: v.x + (atScreenX / v.scale) * (1 - 1 / applied),
        y: v.y + (atScreenY / v.scale) * (1 - 1 / applied),
      };
    });
  },

  panBy(dxScreen: number, dyScreen: number) {
    setViewport((v) => ({ ...v, x: v.x - dxScreen / v.scale, y: v.y - dyScreen / v.scale }));
  },

  fitToPage() {
    const p = page();
    if (!p) return;
    const [cw, ch] = canvasSize();
    const margin = 48;
    const scale = Math.min((cw - margin * 2) / p.width, (ch - margin * 2) / p.height);
    const clamped = m.clamp(scale, 0.02, 4);
    setViewport({
      scale: clamped,
      x: (p.width - cw / clamped) / 2,
      y: (p.height - ch / clamped) / 2,
    });
  },

  zoomToSelection() {
    const bounds = selectionBounds();
    if (!bounds) return actions.fitToPage();
    const [cw, ch] = canvasSize();
    const margin = 80;
    const scale = m.clamp(
      Math.min((cw - margin * 2) / Math.max(bounds.w, 1), (ch - margin * 2) / Math.max(bounds.h, 1)),
      0.02,
      8,
    );
    setViewport({
      scale,
      x: bounds.x + bounds.w / 2 - cw / scale / 2,
      y: bounds.y + bounds.h / 2 - ch / scale / 2,
    });
  },

  setPage(i: number) {
    batch(() => {
      setState("pageIndex", i);
      setState("selection", []);
    });
    actions.fitToPage();
  },

  // -------------------------------------------------------------------------
  // Playback
  // -------------------------------------------------------------------------

  setPlayhead(timelineId: string | null, time: number) {
    setState("playhead", { timelineId, time, playing: false });
  },

  setPlaying(playing: boolean) {
    setState("playhead", "playing", playing);
  },

  dismissError() {
    setState("error", null);
  },

  dismissNotice() {
    setState("notice", null);
  },
};

// ---------------------------------------------------------------------------
// Canvas size, published by the canvas so viewport maths can reach it
// ---------------------------------------------------------------------------

const [canvasSize, setCanvasSize] = createSignal<[number, number]>([1200, 800]);
export { canvasSize, setCanvasSize };
