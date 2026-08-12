/**
 * The canvas: viewport, tools, and the selection overlay.
 *
 * Two rules shape everything here.
 *
 * **Gestures are optimistic, commits are single.** A drag writes to the store's `live`
 * layer and never touches the document; releasing the pointer sends one operation. The
 * canvas therefore runs at pointer speed regardless of round-trip latency, and one drag
 * is one undo step.
 *
 * **The overlay never re-renders the scene.** Handles, marquee and guides live in their
 * own layer above the artwork, so dragging a handle does not invalidate a single node.
 */

import { createEffect, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { edit } from "../ipc";
import { attachGestures, type GestureContext } from "../input";
import * as m from "../math";
import {
  actions,
  canvasSize,
  docToScreen,
  index,
  localBounds,
  page,
  screenToDoc,
  selectionBounds,
  setCanvasSize,
  state,
  tool,
  viewport,
} from "../store";
import type { Node, NodeId, Op } from "../types";
import { SceneNode } from "./render";

type HandleId = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "rotate";

interface DragState {
  kind: "move" | "transform" | "marquee" | "create" | "pan";
  handle?: HandleId;
  origin: m.Rect | null;
  startDoc: [number, number];
  /** Where the person put their finger relative to the thing they grabbed, so the
   *  content does not jump to centre itself under the pointer. */
  grabOffset: [number, number];
}

const HANDLES: { id: HandleId; fx: number; fy: number }[] = [
  { id: "nw", fx: 0, fy: 0 },
  { id: "n", fx: 0.5, fy: 0 },
  { id: "ne", fx: 1, fy: 0 },
  { id: "e", fx: 1, fy: 0.5 },
  { id: "se", fx: 1, fy: 1 },
  { id: "s", fx: 0.5, fy: 1 },
  { id: "sw", fx: 0, fy: 1 },
  { id: "w", fx: 0, fy: 0.5 },
];

export function Canvas(props: { coarse: boolean }) {
  let container!: HTMLDivElement;

  const [drag, setDrag] = createSignal<DragState | null>(null);
  const [marquee, setMarquee] = createSignal<m.Rect | null>(null);
  const [draft, setDraft] = createSignal<m.Rect | null>(null);
  const [penPoints, setPenPoints] = createSignal<[number, number][]>([]);
  const [loupe, setLoupe] = createSignal<{ x: number; y: number } | null>(null);
  const [hovered, setHovered] = createSignal<NodeId | null>(null);

  /** Touch needs bigger targets than a mouse; 11 vs 5 device pixels of radius. */
  const handleRadius = () => (props.coarse ? 11 : 5);

  onMount(() => {
    const observer = new ResizeObserver(() => {
      setCanvasSize([container.clientWidth, container.clientHeight]);
    });
    observer.observe(container);
    setCanvasSize([container.clientWidth, container.clientHeight]);

    const detach = attachGestures(container, {
      onTap: handleTap,
      onDoubleTap: handleDoubleTap,
      onLongPress: handleLongPress,
      onDragStart: handleDragStart,
      onDragMove: handleDragMove,
      onDragEnd: handleDragEnd,
      onZoom: (factor, at) => actions.zoomBy(factor, at.x, at.y),
      onPan: (delta) => actions.panBy(delta.x, delta.y),
      onHover: (at) => setHovered(nodeAt(at.x, at.y)),
    });

    onCleanup(() => {
      observer.disconnect();
      detach();
    });
  });

  // Keep the view sensible when the window shape changes, e.g. a phone rotating.
  createEffect(() => {
    canvasSize();
  });

  // -------------------------------------------------------------------------
  // Hit testing
  // -------------------------------------------------------------------------

  /**
   * Which node is under this point?
   *
   * The browser already knows: `elementFromPoint` respects fills, strokes, opacity and
   * paint order, all of which a hand-rolled test would have to reimplement and get
   * subtly wrong.
   */
  function nodeAt(sx: number, sy: number, deep = false): NodeId | null {
    const rect = container.getBoundingClientRect();
    const el = document.elementFromPoint(rect.left + sx, rect.top + sy);
    const owner = el?.closest("[data-node-id]") as SVGElement | null;
    if (!owner) return null;

    const id = owner.getAttribute("data-node-id");
    if (!id) return null;
    if (deep) return id;

    // A single click picks the outermost thing below the page root — clicking one card
    // in a group selects the group. Double-click goes deeper.
    const p = page();
    if (!p) return id;
    const idx = index();
    let current = id;
    for (;;) {
      const entry = idx.get(current);
      if (!entry?.parent || entry.parent === p.root.id) return current;
      current = entry.parent;
    }
  }

  function handleAt(sx: number, sy: number): HandleId | null {
    const bounds = selectionBounds();
    if (!bounds || state.selection.length === 0) return null;
    const radius = handleRadius() + (props.coarse ? 8 : 3);

    for (const h of HANDLES) {
      const [hx, hy] = docToScreen(bounds.x + bounds.w * h.fx, bounds.y + bounds.h * h.fy);
      if (Math.hypot(sx - hx, sy - hy) <= radius) return h.id;
    }
    const [rx, ry] = docToScreen(bounds.x + bounds.w / 2, bounds.y);
    if (Math.hypot(sx - rx, sy - (ry - 28)) <= radius) return "rotate";
    return null;
  }

  // -------------------------------------------------------------------------
  // Taps
  // -------------------------------------------------------------------------

  function handleTap(at: GestureContext) {
    const active = tool();

    if (active === "text") {
      void createText(at);
      return;
    }
    if (active === "pen") {
      setPenPoints((pts) => [...pts, screenToDoc(at.x, at.y)]);
      return;
    }
    if (active === "hand") return;

    const id = nodeAt(at.x, at.y);
    if (!id) {
      actions.clearSelection();
      return;
    }
    if (at.shift || at.meta) actions.toggleSelect(id);
    else actions.select([id]);
  }

  function handleDoubleTap(at: GestureContext) {
    if (tool() === "pen") {
      void finishPen();
      return;
    }
    const deep = nodeAt(at.x, at.y, true);
    if (deep) actions.select([deep]);
  }

  function handleLongPress(at: GestureContext) {
    // Long press is touch's right-click: add to the selection without needing a
    // modifier key there is no keyboard for.
    const id = nodeAt(at.x, at.y);
    if (id) actions.toggleSelect(id);
  }

  // -------------------------------------------------------------------------
  // Drags
  // -------------------------------------------------------------------------

  function handleDragStart(at: GestureContext) {
    const startDoc = screenToDoc(at.x, at.y);
    const active = tool();

    if (active === "hand") {
      setDrag({ kind: "pan", origin: null, startDoc, grabOffset: [0, 0] });
      return;
    }

    if (active === "rect" || active === "ellipse") {
      setDrag({ kind: "create", origin: null, startDoc, grabOffset: [0, 0] });
      setDraft({ x: startDoc[0], y: startDoc[1], w: 0, h: 0 });
      return;
    }

    const handle = handleAt(at.x, at.y);
    if (handle) {
      setDrag({
        kind: "transform",
        handle,
        origin: selectionBounds(),
        startDoc,
        grabOffset: [0, 0],
      });
      if (props.coarse) setLoupe({ x: at.x, y: at.y });
      return;
    }

    const id = nodeAt(at.x, at.y);
    if (id && (state.selection.includes(id) || state.selection.length === 0)) {
      if (!state.selection.includes(id)) actions.select([id]);
      const bounds = selectionBounds();
      setDrag({
        kind: "move",
        origin: bounds,
        startDoc,
        grabOffset: bounds ? [startDoc[0] - bounds.x, startDoc[1] - bounds.y] : [0, 0],
      });
      return;
    }
    if (id) {
      actions.select([id]);
      const bounds = selectionBounds();
      setDrag({ kind: "move", origin: bounds, startDoc, grabOffset: [0, 0] });
      return;
    }

    setDrag({ kind: "marquee", origin: null, startDoc, grabOffset: [0, 0] });
    setMarquee({ x: startDoc[0], y: startDoc[1], w: 0, h: 0 });
  }

  function handleDragMove(at: GestureContext, delta: { x: number; y: number }, from: { x: number; y: number }) {
    const d = drag();
    if (!d) return;
    const nowDoc = screenToDoc(at.x, at.y);
    if (props.coarse && d.kind === "transform") setLoupe({ x: at.x, y: at.y });

    switch (d.kind) {
      case "pan":
        actions.panBy(delta.x, delta.y);
        break;

      case "move": {
        let dx = nowDoc[0] - d.startDoc[0];
        let dy = nowDoc[1] - d.startDoc[1];
        // Shift constrains to an axis, the way it does in every design tool.
        if (at.shift) {
          if (Math.abs(from.x) > Math.abs(from.y)) dy = 0;
          else dx = 0;
        }
        for (const id of state.selection) actions.setLive(id, m.translate(dx, dy));
        break;
      }

      case "transform": {
        const origin = d.origin;
        if (!origin) break;

        if (d.handle === "rotate") {
          const cx = origin.x + origin.w / 2;
          const cy = origin.y + origin.h / 2;
          const startAngle = Math.atan2(d.startDoc[1] - cy, d.startDoc[0] - cx);
          let angle = Math.atan2(nowDoc[1] - cy, nowDoc[0] - cx) - startAngle;
          // Shift snaps to 15°, which is what makes precise angles reachable at all
          // with a finger.
          if (at.shift) angle = Math.round(angle / (Math.PI / 12)) * (Math.PI / 12);
          const matrix = m.rotateAbout(angle, cx, cy);
          for (const id of state.selection) actions.setLive(id, matrix);
          break;
        }

        const h = HANDLES.find((x) => x.id === d.handle);
        if (!h) break;

        // Scale away from the opposite corner, unless alt is held, which scales about
        // the centre.
        const anchorX = at.alt ? origin.x + origin.w / 2 : origin.x + origin.w * (1 - h.fx);
        const anchorY = at.alt ? origin.y + origin.h / 2 : origin.y + origin.h * (1 - h.fy);

        const spanX = Math.max(Math.abs(d.startDoc[0] - anchorX), 1e-6);
        const spanY = Math.max(Math.abs(d.startDoc[1] - anchorY), 1e-6);

        let sx = h.fx === 0.5 ? 1 : Math.abs(nowDoc[0] - anchorX) / spanX;
        let sy = h.fy === 0.5 ? 1 : Math.abs(nowDoc[1] - anchorY) / spanY;
        if (at.shift && h.fx !== 0.5 && h.fy !== 0.5) {
          const uniform = Math.max(sx, sy);
          sx = uniform;
          sy = uniform;
        }

        const matrix = m.scaleAbout(sx, sy, anchorX, anchorY);
        for (const id of state.selection) actions.setLive(id, matrix);
        break;
      }

      case "marquee":
        setMarquee(rectBetween(d.startDoc, nowDoc));
        break;

      case "create":
        setDraft(rectBetween(d.startDoc, nowDoc, at.shift));
        break;
    }
  }

  function handleDragEnd(_at: GestureContext, cancelled: boolean) {
    const d = drag();
    setDrag(null);
    setLoupe(null);
    if (!d) return;

    if (cancelled) {
      actions.clearLive();
      setMarquee(null);
      setDraft(null);
      return;
    }

    switch (d.kind) {
      case "move":
        void actions.commitLive("Move");
        break;
      case "transform":
        void actions.commitLive(d.handle === "rotate" ? "Rotate" : "Resize");
        break;
      case "marquee": {
        const box = marquee();
        setMarquee(null);
        if (box) actions.select(nodesWithin(box));
        break;
      }
      case "create": {
        const box = draft();
        setDraft(null);
        if (box && box.w > 1 && box.h > 1) void createShape(box);
        break;
      }
    }
  }

  function rectBetween(a: [number, number], b: [number, number], square = false): m.Rect {
    let w = b[0] - a[0];
    let h = b[1] - a[1];
    if (square) {
      const size = Math.max(Math.abs(w), Math.abs(h));
      w = Math.sign(w || 1) * size;
      h = Math.sign(h || 1) * size;
    }
    return { x: Math.min(a[0], a[0] + w), y: Math.min(a[1], a[1] + h), w: Math.abs(w), h: Math.abs(h) };
  }

  function nodesWithin(box: m.Rect): NodeId[] {
    const p = page();
    if (!p) return [];
    const idx = index();
    const hits: NodeId[] = [];
    for (const child of p.root.children ?? []) {
      const entry = idx.get(child.id);
      const local = entry ? localBounds(entry.node) : null;
      if (!entry || !local) continue;
      if (m.rectIntersects(m.transformRect(entry.world, local), box)) hits.push(child.id);
    }
    return hits;
  }

  // -------------------------------------------------------------------------
  // Creating
  // -------------------------------------------------------------------------

  async function createShape(box: m.Rect) {
    const p = page();
    if (!p) return;
    const kind = tool();
    const node: Node = {
      id: "",
      type: kind === "ellipse" ? "ellipse" : "rect",
      name: kind === "ellipse" ? "Ellipse" : "Rectangle",
      width: box.w,
      height: box.h,
      transform: m.translate(box.x, box.y),
      fills: [{ type: "solid", color: "#7c3aed" }],
    };
    await insert(node);
  }

  async function createText(at: GestureContext) {
    const p = page();
    if (!p) return;
    const [x, y] = screenToDoc(at.x, at.y);
    await insert({
      id: "",
      type: "text",
      name: "Text",
      content: "Text",
      fontFamily: "Inter",
      fontSize: 32,
      transform: m.translate(x, y),
      fills: [{ type: "solid", color: "#111111" }],
    });
  }

  async function finishPen() {
    const points = penPoints();
    setPenPoints([]);
    if (points.length < 2) return;

    const d =
      points.map((pt, i) => `${i === 0 ? "M" : "L"} ${m.fmt(pt[0])} ${m.fmt(pt[1])}`).join(" ");

    await insert({
      id: "",
      type: "path",
      name: "Path",
      d,
      strokes: [{ paint: { type: "solid", color: "#111111" }, width: 2 }],
    });
  }

  /** Insert a node, asking the backend for an id so it matches the document's format. */
  async function insert(node: Node) {
    const p = page();
    if (!p) return;
    const [id] = await edit.newIds(1);
    const withId = { ...node, id };
    const op: Op = { op: "node.insert", parent: p.root.id, node: withId };
    await actions.patch([op], `Add ${node.name ?? node.type}`);
    actions.select([id]);
    actions.setTool("select");
  }

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

  const viewTransform = () => {
    const v = viewport();
    return `scale(${v.scale}) translate(${-v.x} ${-v.y})`;
  };

  return (
    <div
      class="canvas"
      ref={container}
      classList={{ "canvas--panning": tool() === "hand" }}
      role="application"
      aria-label="Design canvas"
    >
      <Show when={page()}>
        {(p) => (
          <>
            <svg class="canvas__scene" width="100%" height="100%">
              <g transform={viewTransform()}>
                {/* The page itself, so the artboard edge is visible against the desk. */}
                <rect
                  x={0}
                  y={0}
                  width={p().width}
                  height={p().height}
                  fill={
                    p().background?.type === "solid"
                      ? (p().background as any).color.slice(0, 7)
                      : "#ffffff"
                  }
                  class="canvas__page"
                />
                <SceneNode node={p().root} />
              </g>
            </svg>

            <Overlay
              page={p()}
              handleRadius={handleRadius()}
              marquee={marquee()}
              draft={draft()}
              penPoints={penPoints()}
              hovered={hovered()}
              coarse={props.coarse}
            />

            <Show when={loupe()}>
              {(pos) => <Loupe at={pos()} page={p()} />}
            </Show>
          </>
        )}
      </Show>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

function Overlay(props: {
  page: NonNullable<ReturnType<typeof page>>;
  handleRadius: number;
  marquee: m.Rect | null;
  draft: m.Rect | null;
  penPoints: [number, number][];
  hovered: NodeId | null;
  coarse: boolean;
}) {
  const bounds = () => selectionBounds();

  const screenRect = (r: m.Rect) => {
    const [x, y] = docToScreen(r.x, r.y);
    const scale = viewport().scale;
    return { x, y, w: r.w * scale, h: r.h * scale };
  };

  const hoveredBounds = () => {
    const id = props.hovered;
    if (!id || state.selection.includes(id)) return null;
    const entry = index().get(id);
    const local = entry ? localBounds(entry.node) : null;
    if (!entry || !local) return null;
    return screenRect(m.transformRect(entry.world, local));
  };

  return (
    <svg class="canvas__overlay" width="100%" height="100%">
      <Show when={hoveredBounds()}>
        {(r) => (
          <rect
            x={r().x}
            y={r().y}
            width={r().w}
            height={r().h}
            class="overlay__hover"
          />
        )}
      </Show>

      <Show when={bounds()}>
        {(b) => {
          const r = () => screenRect(b());
          return (
            <>
              <rect x={r().x} y={r().y} width={r().w} height={r().h} class="overlay__selection" />

              {/* Rotate handle, held off the top edge so it does not collide with 'n'. */}
              <line
                x1={r().x + r().w / 2}
                y1={r().y}
                x2={r().x + r().w / 2}
                y2={r().y - 28}
                class="overlay__tether"
              />
              <circle
                cx={r().x + r().w / 2}
                cy={r().y - 28}
                r={props.handleRadius}
                class="overlay__handle overlay__handle--rotate"
              />

              <For each={HANDLES}>
                {(h) => (
                  <circle
                    cx={r().x + r().w * h.fx}
                    cy={r().y + r().h * h.fy}
                    r={props.handleRadius}
                    class="overlay__handle"
                  />
                )}
              </For>

              <text x={r().x} y={r().y - 8} class="overlay__readout">
                {m.fmt(b().w)} × {m.fmt(b().h)}
              </text>
            </>
          );
        }}
      </Show>

      <Show when={props.marquee}>
        {(box) => {
          const r = () => screenRect(box());
          return (
            <rect x={r().x} y={r().y} width={r().w} height={r().h} class="overlay__marquee" />
          );
        }}
      </Show>

      <Show when={props.draft}>
        {(box) => {
          const r = () => screenRect(box());
          return <rect x={r().x} y={r().y} width={r().w} height={r().h} class="overlay__draft" />;
        }}
      </Show>

      <Show when={props.penPoints.length > 0}>
        <polyline
          points={props.penPoints.map((p) => docToScreen(p[0], p[1]).join(",")).join(" ")}
          class="overlay__pen"
        />
        <For each={props.penPoints}>
          {(p) => {
            const s = () => docToScreen(p[0], p[1]);
            return <circle cx={s()[0]} cy={s()[1]} r={props.handleRadius} class="overlay__handle" />;
          }}
        </For>
      </Show>
    </svg>
  );
}

/**
 * Magnifier shown while a finger drags a handle.
 *
 * Fingers are opaque and about a centimetre wide, so the one thing a person needs to
 * see while placing a point is the thing their hand is covering. The loupe sits above
 * and to the left of the contact point and shows the artwork at four times scale.
 */
function Loupe(props: { at: { x: number; y: number }; page: NonNullable<ReturnType<typeof page>> }) {
  const SIZE = 132;
  const ZOOM = 4;

  const origin = () => {
    const [dx, dy] = screenToDoc(props.at.x, props.at.y);
    return [dx, dy] as const;
  };

  // Flip to the other side near an edge, so the loupe never leaves the screen.
  const placement = () => {
    const [cw] = canvasSize();
    const left = props.at.x > SIZE + 24 ? props.at.x - SIZE - 16 : props.at.x + 16;
    const top = props.at.y > SIZE + 24 ? props.at.y - SIZE - 16 : props.at.y + 16;
    return { left: `${Math.min(left, cw - SIZE - 8)}px`, top: `${Math.max(top, 8)}px` };
  };

  const scale = () => viewport().scale * ZOOM;

  return (
    <div class="loupe" style={{ ...placement(), width: `${SIZE}px`, height: `${SIZE}px` }}>
      <svg width={SIZE} height={SIZE}>
        <g
          transform={`translate(${SIZE / 2} ${SIZE / 2}) scale(${scale()}) translate(${-origin()[0]} ${-origin()[1]})`}
        >
          <rect
            x={0}
            y={0}
            width={props.page.width}
            height={props.page.height}
            fill={
              props.page.background?.type === "solid"
                ? (props.page.background as any).color.slice(0, 7)
                : "#ffffff"
            }
          />
          <SceneNode node={props.page.root} />
        </g>
        <line x1={SIZE / 2 - 8} y1={SIZE / 2} x2={SIZE / 2 + 8} y2={SIZE / 2} class="loupe__cross" />
        <line x1={SIZE / 2} y1={SIZE / 2 - 8} x2={SIZE / 2} y2={SIZE / 2 + 8} class="loupe__cross" />
      </svg>
    </div>
  );
}
