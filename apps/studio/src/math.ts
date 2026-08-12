/**
 * Matrix and rectangle helpers.
 *
 * Mirrors `md-doc`'s `Transform`, using the same `[a, b, c, d, e, f]` layout, so a
 * matrix computed on the canvas can go straight into a `node.update` with no
 * conversion — and so the two cannot disagree about composition order.
 */

import type { Matrix } from "./types";

export const IDENTITY: Matrix = [1, 0, 0, 1, 0, 0];

export function isIdentity(m: Matrix): boolean {
  return IDENTITY.every((v, i) => Math.abs(m[i] - v) < 1e-12);
}

export function translate(x: number, y: number): Matrix {
  return [1, 0, 0, 1, x, y];
}

export function scale(sx: number, sy: number): Matrix {
  return [sx, 0, 0, sy, 0, 0];
}

export function rotate(radians: number): Matrix {
  const s = Math.sin(radians);
  const c = Math.cos(radians);
  return [c, s, -s, c, 0, 0];
}

/** `a` applied first, then `b`. Same convention as `Transform::then`. */
export function then(a: Matrix, b: Matrix): Matrix {
  const [a1, b1, c1, d1, e1, f1] = a;
  const [a2, b2, c2, d2, e2, f2] = b;
  return [
    a1 * a2 + b1 * c2,
    a1 * b2 + b1 * d2,
    c1 * a2 + d1 * c2,
    c1 * b2 + d1 * d2,
    e1 * a2 + f1 * c2 + e2,
    e1 * b2 + f1 * d2 + f2,
  ];
}

export function apply(m: Matrix, x: number, y: number): [number, number] {
  return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
}

export function invert(m: Matrix): Matrix | null {
  const det = m[0] * m[3] - m[1] * m[2];
  if (Math.abs(det) < 1e-12) return null;
  return [
    m[3] / det,
    -m[1] / det,
    -m[2] / det,
    m[0] / det,
    (m[2] * m[5] - m[3] * m[4]) / det,
    (m[1] * m[4] - m[0] * m[5]) / det,
  ];
}

/** Rotate about a point — what a rotate handle needs, since people turn things around
 *  a selection's centre rather than around the origin. */
export function rotateAbout(radians: number, cx: number, cy: number): Matrix {
  return then(then(translate(-cx, -cy), rotate(radians)), translate(cx, cy));
}

export function scaleAbout(sx: number, sy: number, cx: number, cy: number): Matrix {
  return then(then(translate(-cx, -cy), scale(sx, sy)), translate(cx, cy));
}

export interface Decomposed {
  x: number;
  y: number;
  rotation: number;
  scaleX: number;
  scaleY: number;
}

/** Pull a matrix apart into the numbers a person can type into a box. */
export function decompose(m: Matrix): Decomposed {
  const [a, b, c, d, e, f] = m;
  const scaleX = Math.hypot(a, b);
  const rotation = Math.atan2(b, a);
  const shear = a * c + b * d;
  const scaleYsq = c * c + d * d - (scaleX > 0 ? (shear * shear) / (scaleX * scaleX) : 0);
  let scaleY = Math.sqrt(Math.max(scaleYsq, 0));
  if (a * d - b * c < 0) scaleY = -scaleY;
  return { x: e, y: f, rotation, scaleX, scaleY };
}

export function toSvg(m: Matrix): string {
  return `matrix(${m.map(fmt).join(" ")})`;
}

/** Four decimal places, trailing zeros stripped — the same format the document uses,
 *  so a value written from here does not dirty the file with a different spelling. */
export function fmt(v: number): string {
  if (!Number.isFinite(v)) return "0";
  let s = v.toFixed(4);
  if (s.includes(".")) s = s.replace(/0+$/, "").replace(/\.$/, "");
  return s === "-0" ? "0" : s;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function rectUnion(a: Rect, b: Rect): Rect {
  const x0 = Math.min(a.x, b.x);
  const y0 = Math.min(a.y, b.y);
  const x1 = Math.max(a.x + a.w, b.x + b.w);
  const y1 = Math.max(a.y + a.h, b.y + b.h);
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
}

export function rectContains(r: Rect, x: number, y: number): boolean {
  return x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
}

export function rectIntersects(a: Rect, b: Rect): boolean {
  return !(a.x + a.w < b.x || b.x + b.w < a.x || a.y + a.h < b.y || b.y + b.h < a.y);
}

/** Axis-aligned box containing a rectangle after a transform. A rotated box's extent is
 *  not the transform of its extent, so all four corners have to be mapped. */
export function transformRect(m: Matrix, r: Rect): Rect {
  const corners: [number, number][] = [
    apply(m, r.x, r.y),
    apply(m, r.x + r.w, r.y),
    apply(m, r.x + r.w, r.y + r.h),
    apply(m, r.x, r.y + r.h),
  ];
  const xs = corners.map((c) => c[0]);
  const ys = corners.map((c) => c[1]);
  const x0 = Math.min(...xs);
  const y0 = Math.min(...ys);
  return { x: x0, y: Math.min(...ys), w: Math.max(...xs) - x0, h: Math.max(...ys) - y0 };
}

export function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

export function degrees(radians: number): number {
  return (radians * 180) / Math.PI;
}

export function radians(deg: number): number {
  return (deg * Math.PI) / 180;
}
