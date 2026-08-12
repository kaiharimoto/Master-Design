/**
 * One input layer for mouse, pen and touch.
 *
 * Everything above this file sees gestures, not events. That is what lets the same
 * tool code run under a mouse on Windows and a thumb on Android without branching, and
 * it is where the differences that *do* matter get handled once:
 *
 * - Two fingers mean pinch-zoom and pan together; a wheel means zoom, and a trackpad's
 *   two-finger swipe means pan. Both arrive here as the same `zoom`/`pan` calls.
 * - Touch gets slop before a tap becomes a drag, because fingers move a few pixels on
 *   the way down and a design tool that nudges a layer every time you tap it is
 *   maddening.
 * - Pen pressure and tilt are passed through where the hardware reports them.
 */

export interface Point {
  x: number;
  y: number;
}

export type PointerKind = "mouse" | "pen" | "touch";

export interface GestureContext extends Point {
  kind: PointerKind;
  /** Modifier keys, meaningless on touch but harmless there. */
  shift: boolean;
  alt: boolean;
  ctrl: boolean;
  meta: boolean;
  pressure: number;
  /** True while a second finger is down, so tools can bail out of a drag. */
  multiTouch: boolean;
}

export interface GestureHandlers {
  onTap?(at: GestureContext): void;
  onDoubleTap?(at: GestureContext): void;
  onLongPress?(at: GestureContext): void;
  onDragStart?(at: GestureContext): void;
  onDragMove?(at: GestureContext, delta: Point, fromStart: Point): void;
  onDragEnd?(at: GestureContext, cancelled: boolean): void;
  onZoom?(factor: number, at: Point): void;
  onPan?(delta: Point): void;
  onHover?(at: GestureContext): void;
}

/** How far a pointer may move before it counts as a drag rather than a tap. */
const SLOP = { mouse: 3, pen: 4, touch: 10 } as const;

const LONG_PRESS_MS = 500;
const DOUBLE_TAP_MS = 300;
const DOUBLE_TAP_SLOP = 24;

export function attachGestures(element: HTMLElement, handlers: GestureHandlers): () => void {
  const active = new Map<number, { start: Point; last: Point; kind: PointerKind }>();

  let dragging = false;
  let start: Point = { x: 0, y: 0 };
  let longPressTimer: number | undefined;
  let lastTap = { time: 0, x: 0, y: 0 };
  let pinchDistance = 0;
  let pinchCentre: Point = { x: 0, y: 0 };

  const local = (e: PointerEvent | WheelEvent): Point => {
    const rect = element.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  };

  const context = (e: PointerEvent): GestureContext => {
    const p = local(e);
    return {
      ...p,
      kind: (e.pointerType as PointerKind) || "mouse",
      shift: e.shiftKey,
      alt: e.altKey,
      ctrl: e.ctrlKey,
      meta: e.metaKey,
      pressure: e.pressure || (e.pointerType === "mouse" ? 0.5 : 0),
      multiTouch: active.size > 1,
    };
  };

  const cancelLongPress = () => {
    if (longPressTimer !== undefined) {
      window.clearTimeout(longPressTimer);
      longPressTimer = undefined;
    }
  };

  const onPointerDown = (e: PointerEvent) => {
    const p = local(e);
    active.set(e.pointerId, { start: p, last: p, kind: (e.pointerType as PointerKind) || "mouse" });

    if (active.size === 2) {
      // A second finger cancels whatever the first was doing. Continuing the drag while
      // pinching produces a layer that lurches sideways as the view scales.
      cancelLongPress();
      if (dragging) {
        handlers.onDragEnd?.(context(e), true);
        dragging = false;
      }
      const [a, b] = [...active.values()];
      pinchDistance = Math.hypot(a.last.x - b.last.x, a.last.y - b.last.y);
      pinchCentre = { x: (a.last.x + b.last.x) / 2, y: (a.last.y + b.last.y) / 2 };
      return;
    }

    if (active.size > 2) return;

    element.setPointerCapture(e.pointerId);
    start = p;

    // Long-press is the touch equivalent of a right-click. Pointless with a mouse,
    // where the context menu already exists.
    if (e.pointerType === "touch") {
      longPressTimer = window.setTimeout(() => {
        if (!dragging && active.size === 1) {
          handlers.onLongPress?.(context(e));
          navigator.vibrate?.(8);
        }
      }, LONG_PRESS_MS);
    }
  };

  const onPointerMove = (e: PointerEvent) => {
    const entry = active.get(e.pointerId);
    const p = local(e);

    if (!entry) {
      handlers.onHover?.(context(e));
      return;
    }

    const previous = entry.last;
    entry.last = p;

    if (active.size >= 2) {
      const [a, b] = [...active.values()];
      const distance = Math.hypot(a.last.x - b.last.x, a.last.y - b.last.y);
      const centre = { x: (a.last.x + b.last.x) / 2, y: (a.last.y + b.last.y) / 2 };

      if (pinchDistance > 0 && distance > 0) {
        handlers.onZoom?.(distance / pinchDistance, centre);
      }
      // Pinching and panning are one gesture: the hand moves while the fingers spread.
      handlers.onPan?.({ x: centre.x - pinchCentre.x, y: centre.y - pinchCentre.y });

      pinchDistance = distance;
      pinchCentre = centre;
      return;
    }

    const slop = SLOP[entry.kind] ?? 4;
    if (!dragging && Math.hypot(p.x - start.x, p.y - start.y) > slop) {
      cancelLongPress();
      dragging = true;
      handlers.onDragStart?.({ ...context(e), x: start.x, y: start.y });
    }

    if (dragging) {
      handlers.onDragMove?.(
        context(e),
        { x: p.x - previous.x, y: p.y - previous.y },
        { x: p.x - start.x, y: p.y - start.y },
      );
    }
  };

  const onPointerUp = (e: PointerEvent) => {
    const entry = active.get(e.pointerId);
    active.delete(e.pointerId);
    cancelLongPress();

    if (element.hasPointerCapture?.(e.pointerId)) {
      element.releasePointerCapture(e.pointerId);
    }

    if (active.size === 1) {
      // Lifting one finger of a pinch: re-anchor so the remaining finger does not
      // teleport the view.
      const [remaining] = [...active.values()];
      pinchCentre = remaining.last;
      pinchDistance = 0;
      return;
    }

    if (!entry) return;

    if (dragging) {
      dragging = false;
      handlers.onDragEnd?.(context(e), false);
      return;
    }

    const now = Date.now();
    const ctx = context(e);
    const isDouble =
      now - lastTap.time < DOUBLE_TAP_MS &&
      Math.hypot(ctx.x - lastTap.x, ctx.y - lastTap.y) < DOUBLE_TAP_SLOP;

    if (isDouble) {
      lastTap = { time: 0, x: 0, y: 0 };
      handlers.onDoubleTap?.(ctx);
    } else {
      lastTap = { time: now, x: ctx.x, y: ctx.y };
      handlers.onTap?.(ctx);
    }
  };

  const onPointerCancel = (e: PointerEvent) => {
    active.delete(e.pointerId);
    cancelLongPress();
    if (dragging) {
      dragging = false;
      handlers.onDragEnd?.(context(e), true);
    }
  };

  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    const at = local(e);

    // A trackpad pinch arrives as a wheel event with ctrlKey set — a browser convention
    // rather than the user holding anything down.
    if (e.ctrlKey || e.metaKey) {
      handlers.onZoom?.(Math.exp(-e.deltaY * 0.01), at);
      return;
    }
    // A mouse wheel zooms, because on a canvas that is what a wheel is for. A trackpad
    // two-finger swipe pans, which is what its horizontal component gives it away as.
    if (e.deltaX === 0 && Math.abs(e.deltaY) > 40 && e.deltaMode === 0) {
      handlers.onZoom?.(Math.exp(-e.deltaY * 0.002), at);
      return;
    }
    handlers.onPan?.({ x: -e.deltaX, y: -e.deltaY });
  };

  const onContextMenu = (e: Event) => e.preventDefault();

  element.addEventListener("pointerdown", onPointerDown);
  element.addEventListener("pointermove", onPointerMove);
  element.addEventListener("pointerup", onPointerUp);
  element.addEventListener("pointercancel", onPointerCancel);
  element.addEventListener("wheel", onWheel, { passive: false });
  element.addEventListener("contextmenu", onContextMenu);

  return () => {
    cancelLongPress();
    element.removeEventListener("pointerdown", onPointerDown);
    element.removeEventListener("pointermove", onPointerMove);
    element.removeEventListener("pointerup", onPointerUp);
    element.removeEventListener("pointercancel", onPointerCancel);
    element.removeEventListener("wheel", onWheel);
    element.removeEventListener("contextmenu", onContextMenu);
  };
}

/**
 * Which shell to show.
 *
 * Chosen from the shape of the window and whether the primary input can hover, rather
 * than from a user-agent string: a Windows tablet in portrait with a stylus wants the
 * touch layout, and a phone in a landscape dock does not.
 */
export type ShellKind = "desktop" | "tablet" | "phone";

export function shellFor(width: number, height: number, coarse: boolean): ShellKind {
  const portrait = height > width;
  if (width < 720 || (portrait && width < 900)) return "phone";
  if (coarse || width < 1200) return "tablet";
  return "desktop";
}

export function prefersCoarsePointer(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(pointer: coarse)").matches;
}
