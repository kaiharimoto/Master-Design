/**
 * Every call into the Rust side.
 *
 * The backend owns the document. The frontend holds a reactive mirror of it and asks
 * for changes — it never edits its copy directly. That is what keeps one validator, one
 * undo stack and one canonical serializer in the system rather than two of each that
 * have to agree, and it is what lets an AI patch arriving over MCP and a mouse drag be
 * genuinely the same kind of event.
 *
 * The cost is a round trip per change, which is why drags are optimistic on the canvas
 * and commit exactly one operation when the pointer lifts. See `store.ts`.
 */

import type {
  AnimationPackage,
  Document,
  EditorState,
  Op,
  PatchReport,
  Timeline,
  UpdateInfo,
} from "./types";

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

let invokeFn: Invoke | null = null;
let invokeReady: Promise<Invoke | null> | null = null;

/** Is a Tauri backend present? False in a plain browser tab. */
export function hasBackend(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

async function getInvoke(): Promise<Invoke> {
  if (invokeFn) return invokeFn;
  if (!invokeReady) {
    invokeReady = hasBackend()
      ? import("@tauri-apps/api/core").then((m) => m.invoke as Invoke)
      : Promise.resolve(null);
  }
  const fn = await invokeReady;
  if (!fn) {
    throw new BackendUnavailable();
  }
  invokeFn = fn;
  return fn;
}

/**
 * Thrown when the app is running outside the desktop or mobile shell.
 *
 * Deliberately not papered over with a JavaScript stand-in. A mock backend would be a
 * second implementation of the patch protocol, which is exactly the duplication this
 * architecture exists to avoid — and one that would quietly disagree with the real one.
 */
export class BackendUnavailable extends Error {
  constructor() {
    super("The Master Design backend is not running. Open the app rather than the dev server.");
    this.name = "BackendUnavailable";
  }
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = await getInvoke();
  return invoke<T>(cmd, args);
}

// ---------------------------------------------------------------------------
// Project
// ---------------------------------------------------------------------------

export const project = {
  /** Current state, including whether there is a project open at all. */
  state: () => call<EditorState>("editor_state"),

  open: (path: string) => call<EditorState>("project_open", { path }),

  create: (path: string, name: string, width: number, height: number) =>
    call<EditorState>("project_create", { path, name, width, height }),

  save: () => call<EditorState>("project_save"),

  /** Reload from disk — how an edit made over MCP reaches the canvas. */
  reload: () => call<EditorState>("project_reload"),

  export: (out?: string) => call<{ files: string[]; bytes: number; warnings: string[] }>(
    "project_export",
    { out: out ?? null },
  ),
};

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

export const edit = {
  /** Apply operations as one undoable step. */
  patch: (ops: Op[], label: string) =>
    call<{ state: EditorState; report: PatchReport }>("doc_patch", { ops, label }),

  undo: () => call<EditorState>("doc_undo"),
  redo: () => call<EditorState>("doc_redo"),

  /** Resolve a selector against the open document. */
  query: (selector: string, page?: string) =>
    call<string[]>("doc_query", { selector, page: page ?? null }),

  /** Fresh node ids, minted by the backend so they match the document's format. */
  newIds: (count: number) => call<string[]>("doc_new_ids", { count }),
};

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/**
 * Geometry that the DOM cannot answer.
 *
 * Hit-testing and measurement are *not* here: `SVGGeometryElement.isPointInFill`,
 * `isPointInStroke`, `getBBox` and `getTotalLength` do those natively and faster than
 * any round trip could. What is left are the operations with no browser equivalent —
 * booleans, stroke outlining, curve fitting, live corners — and those happen on a click
 * rather than per frame, so a round trip costs nothing anyone can perceive.
 *
 * `md-geom` also compiles to WebAssembly, which is what per-frame work (freehand curve
 * fitting as the finger moves) will use when it lands.
 */
export const geom = {
  rectPath: (w: number, h: number, radii: [number, number, number, number]) =>
    call<string>("geom_rect_path", { w, h, radii }),

  ellipsePath: (w: number, h: number) => call<string>("geom_ellipse_path", { w, h }),

  polygonPath: (radius: number, sides: number, rotation: number) =>
    call<string>("geom_polygon_path", { radius, sides, rotation }),

  starPath: (outer: number, inner: number, points: number, rotation: number) =>
    call<string>("geom_star_path", { outer, inner, points, rotation }),

  roundCorners: (d: string, radius: number) => call<string>("geom_round_corners", { d, radius }),

  outlineStroke: (d: string, width: number) => call<string>("geom_outline_stroke", { d, width }),

  /** Fit a freehand point run to bezier curves. */
  fitFreehand: (points: number[], tolerance: number) =>
    call<string>("geom_fit_freehand", { points, tolerance }),
};

// ---------------------------------------------------------------------------
// Animation
// ---------------------------------------------------------------------------

export const anim = {
  list: () => call<AnimationPackage[]>("anim_list"),

  /** Bake a package against a selector, without adding it to the document yet. */
  preview: (pkg: string, selector: string, params: Record<string, unknown>, page: string) =>
    call<Timeline>("anim_bake", { package: pkg, selector, params, page }),

  /** Re-bake an existing timeline after its parameters or the document changed. */
  rebake: (timelineId: string, page: string, params: Record<string, unknown>) =>
    call<Timeline>("anim_rebake", { timelineId, page, params }),
};

// ---------------------------------------------------------------------------
// The bridge to the AI
// ---------------------------------------------------------------------------

export const bridge = {
  /**
   * Publish what the person is looking at, so an agent attached over MCP can answer
   * "make this bounce" without being told what "this" is.
   */
  publishSelection: (
    page: string,
    nodes: string[],
    note: string,
    viewport: [number, number, number, number] | null,
  ) => call<void>("selection_publish", { page, nodes, note, viewport }),

  /** Queue a note for a session that is not attached yet — the phone's path. */
  queueRequest: (note: string, page: string, nodes: string[]) =>
    call<string>("request_queue", { note, page, nodes }),

  /** How to attach an agent to this project, for the UI to show. */
  mcpCommand: () => call<string>("mcp_command"),
};

// ---------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------

export const updates = {
  check: () => call<UpdateInfo>("update_check"),
  install: () => call<void>("update_install"),
  openReleasePage: () => call<void>("update_open_release_page"),
};

// ---------------------------------------------------------------------------
// Snapshot, for previews and for the animation picker's thumbnails
// ---------------------------------------------------------------------------

export const preview = {
  /** Base64 PNG of a page, optionally at a moment in its animation. */
  snapshot: (page: string, time: number | null, width: number, nodeId?: string) =>
    call<string>("doc_snapshot", { page, time, width, nodeId: nodeId ?? null }),
};

export type { Document, EditorState };
