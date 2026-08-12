/**
 * The scene renderer.
 *
 * Emits the same SVG primitives `md-emit` writes at export time, so the canvas is not
 * an approximation of the output — it is the output, in a live DOM. There are
 * unavoidably two renderers (this one is reactive and incremental, the exporter builds
 * a string), and they have to agree; the shared vocabulary of `md-doc` node kinds is
 * what keeps them honest, and any divergence shows up immediately as a design that
 * looks different once exported.
 *
 * Solid's fine-grained reactivity is what makes this viable at scale: changing one
 * node's fill updates one attribute, with no diff over a tree of thousands.
 */

import { For, Show } from "solid-js";
import { fmt, toSvg } from "../math";
import { state } from "../store";
import type { Effect, Node, Paint, Stroke } from "../types";

/** Stable, collision-free id for a gradient or filter belonging to one node. */
function defId(nodeId: string, kind: string, i: number) {
  return `md-${kind}-${nodeId}-${i}`;
}

export function paintValue(paint: Paint | undefined, nodeId: string, i: number): string {
  if (!paint) return "none";
  switch (paint.type) {
    case "solid":
      return paint.color.slice(0, 7);
    case "linearGradient":
      return `url(#${defId(nodeId, "lg", i)})`;
    case "radialGradient":
      return `url(#${defId(nodeId, "rg", i)})`;
    case "image":
      return `url(#${defId(nodeId, "pat", i)})`;
  }
}

function paintOpacity(paint: Paint | undefined): number {
  if (!paint) return 1;
  const base = paint.opacity ?? 1;
  // A hex with alpha carries its own transparency; multiply the two rather than
  // letting one silently win.
  if (paint.type === "solid" && paint.color.length === 9) {
    return base * (parseInt(paint.color.slice(7, 9), 16) / 255);
  }
  return base;
}

function PaintDefs(props: { node: Node }) {
  const paints = () => [
    ...(props.node.fills ?? []).map((p, i) => ({ paint: p, i })),
    ...(props.node.strokes ?? []).map((s, i) => ({ paint: s.paint, i: 1000 + i })),
  ];

  return (
    <For each={paints()}>
      {(entry) => (
        <Show when={entry.paint.type !== "solid"}>
          <Show when={entry.paint.type === "linearGradient" && entry.paint}>
            {(p) => (
              <linearGradient
                id={defId(props.node.id, "lg", entry.i)}
                x1={p().type === "linearGradient" ? (p() as any).from[0] : 0}
                y1={p().type === "linearGradient" ? (p() as any).from[1] : 0}
                x2={p().type === "linearGradient" ? (p() as any).to[0] : 1}
                y2={p().type === "linearGradient" ? (p() as any).to[1] : 1}
              >
                <For each={(p() as any).stops}>
                  {(stop: { offset: number; color: string }) => (
                    <stop offset={stop.offset} stop-color={stop.color.slice(0, 7)} />
                  )}
                </For>
              </linearGradient>
            )}
          </Show>
          <Show when={entry.paint.type === "radialGradient" && entry.paint}>
            {(p) => (
              <radialGradient
                id={defId(props.node.id, "rg", entry.i)}
                cx={(p() as any).center[0]}
                cy={(p() as any).center[1]}
                r={(p() as any).radius}
              >
                <For each={(p() as any).stops}>
                  {(stop: { offset: number; color: string }) => (
                    <stop offset={stop.offset} stop-color={stop.color.slice(0, 7)} />
                  )}
                </For>
              </radialGradient>
            )}
          </Show>
        </Show>
      )}
    </For>
  );
}

function EffectDefs(props: { node: Node }) {
  const effects = () => props.node.effects ?? [];
  return (
    <Show when={effects().length > 0}>
      <filter
        id={defId(props.node.id, "fx", 0)}
        x="-50%"
        y="-50%"
        width="200%"
        height="200%"
      >
        <For each={effects()}>
          {(effect: Effect) => (
            <>
              <Show when={effect.type === "blur" && effect}>
                {(e) => <feGaussianBlur stdDeviation={(e() as any).radius / 2} />}
              </Show>
              <Show when={effect.type === "dropShadow" && effect}>
                {(e) => (
                  <feDropShadow
                    dx={(e() as any).dx}
                    dy={(e() as any).dy}
                    stdDeviation={(e() as any).blur / 2}
                    flood-color={(e() as any).color.slice(0, 7)}
                  />
                )}
              </Show>
            </>
          )}
        </For>
      </filter>
    </Show>
  );
}

function strokeAttrs(stroke: Stroke, nodeId: string, i: number) {
  return {
    stroke: paintValue(stroke.paint, nodeId, 1000 + i),
    "stroke-width": stroke.width,
    "stroke-opacity": paintOpacity(stroke.paint),
    "stroke-linecap": stroke.cap ?? "butt",
    "stroke-linejoin": stroke.join ?? "miter",
    "stroke-dasharray": stroke.dash?.length ? stroke.dash.join(" ") : undefined,
    "stroke-dashoffset": stroke.dashOffset || undefined,
    fill: "none",
  };
}

/** Geometry as SVG path data, for the shapes whose outline the document implies. */
export function geometryPath(node: Node): string | null {
  switch (node.type) {
    case "path":
      return node.d ?? null;
    case "rect":
    case "frame":
      return roundedRectPath(node.width ?? 0, node.height ?? 0, node.cornerRadius);
    case "ellipse": {
      const rx = (node.width ?? 0) / 2;
      const ry = (node.height ?? 0) / 2;
      if (rx <= 0 || ry <= 0) return null;
      // Two arcs, which is all an ellipse needs and what every renderer produces.
      return `M 0 ${fmt(ry)} A ${fmt(rx)} ${fmt(ry)} 0 1 0 ${fmt(rx * 2)} ${fmt(
        ry,
      )} A ${fmt(rx)} ${fmt(ry)} 0 1 0 0 ${fmt(ry)} Z`;
    }
    case "image":
      return roundedRectPath(node.width ?? 0, node.height ?? 0);
    default:
      return null;
  }
}

function roundedRectPath(w: number, h: number, radii?: [number, number, number, number]): string {
  if (w <= 0 || h <= 0) return "";
  const cap = Math.min(w, h) / 2;
  const [tl, tr, br, bl] = (radii ?? [0, 0, 0, 0]).map((r) =>
    Math.max(0, Math.min(r, cap)),
  ) as [number, number, number, number];

  if (tl === 0 && tr === 0 && br === 0 && bl === 0) {
    return `M 0 0 L ${fmt(w)} 0 L ${fmt(w)} ${fmt(h)} L 0 ${fmt(h)} Z`;
  }
  const a = (r: number) => `A ${fmt(r)} ${fmt(r)} 0 0 1`;
  return [
    `M 0 ${fmt(tl)}`,
    `${a(tl)} ${fmt(tl)} 0`,
    `L ${fmt(w - tr)} 0`,
    `${a(tr)} ${fmt(w)} ${fmt(tr)}`,
    `L ${fmt(w)} ${fmt(h - br)}`,
    `${a(br)} ${fmt(w - br)} ${fmt(h)}`,
    `L ${fmt(bl)} ${fmt(h)}`,
    `${a(bl)} 0 ${fmt(h - bl)}`,
    "Z",
  ].join(" ");
}

function TextNode(props: { node: Node }) {
  const lines = () => (props.node.content ?? "").split("\n");
  const size = () => props.node.fontSize ?? 16;
  const lineHeight = () => (props.node.lineHeight ?? 1.4) * size();
  const anchor = () =>
    props.node.align === "center" ? "middle" : props.node.align === "right" ? "end" : "start";
  const x = () =>
    props.node.align === "center"
      ? (props.node.width ?? 0) / 2
      : props.node.align === "right"
        ? (props.node.width ?? 0)
        : 0;

  return (
    <text
      font-family={`${props.node.fontFamily ?? "Inter"}, system-ui, sans-serif`}
      font-size={String(size())}
      font-weight={String(props.node.fontWeight ?? 400)}
      font-style={props.node.italic ? "italic" : undefined}
      letter-spacing={
        props.node.letterSpacing ? String(props.node.letterSpacing * size()) : undefined
      }
      text-anchor={anchor()}
      fill={paintValue(props.node.fills?.[0], props.node.id, 0)}
      fill-opacity={paintOpacity(props.node.fills?.[0])}
      style={{
        "text-transform":
          props.node.textCase && props.node.textCase !== "original"
            ? props.node.textCase === "upper"
              ? "uppercase"
              : props.node.textCase === "lower"
                ? "lowercase"
                : "capitalize"
            : undefined,
        // Text is decoration on the canvas; selecting it would fight the marquee.
        "user-select": "none",
      }}
    >
      <For each={lines()}>
        {(line, i) => (
          <tspan x={x()} y={size() * 0.8 + lineHeight() * i()}>
            {line}
          </tspan>
        )}
      </For>
    </text>
  );
}

export function SceneNode(props: { node: Node }) {
  const node = () => props.node;
  const live = () => state.live[node().id];

  // The live transform is a separate wrapper rather than a modified `transform`, so a
  // drag never has to rewrite the document's own matrix — and so releasing the pointer
  // commits one clean operation.
  const transform = () => {
    const base = node().transform;
    return base && base.some((v, i) => v !== [1, 0, 0, 1, 0, 0][i]) ? toSvg(base) : undefined;
  };

  const path = () => geometryPath(node());
  const isContainer = () => node().type === "frame" || node().type === "group";

  return (
    <Show when={node().visible !== false}>
      <g
        data-node-id={node().id}
        transform={transform()}
        opacity={node().opacity ?? 1}
        filter={node().effects?.length ? `url(#${defId(node().id, "fx", 0)})` : undefined}
        style={{
          "mix-blend-mode": (node().blendMode as any) ?? undefined,
          // Locked layers must not swallow clicks meant for what is underneath.
          "pointer-events": node().locked ? "none" : undefined,
        }}
      >
        <PaintDefs node={node()} />
        <EffectDefs node={node()} />

        <g transform={live() ? toSvg(live()!) : undefined}>
          <Show when={node().type === "text"}>
            <TextNode node={node()} />
          </Show>

          <Show when={node().type === "image"}>
            <image
              href={node().asset}
              width={node().width}
              height={node().height}
              preserveAspectRatio={node().fit === "contain" ? "xMidYMid meet" : "xMidYMid slice"}
            />
          </Show>

          <Show when={path()}>
            {(d) => (
              <>
                <For each={node().fills ?? []}>
                  {(paint, i) => (
                    <path
                      d={d()}
                      fill={paintValue(paint, node().id, i())}
                      fill-opacity={paintOpacity(paint)}
                      fill-rule={node().fillRule === "evenOdd" ? "evenodd" : undefined}
                    />
                  )}
                </For>
                <For each={node().strokes ?? []}>
                  {(stroke, i) => <path d={d()} {...strokeAttrs(stroke, node().id, i())} />}
                </For>
                {/*
                  An unfilled, unstroked shape still has to be clickable, or a frame used
                  purely as a container could never be selected on the canvas.
                */}
                <Show when={!node().fills?.length && !node().strokes?.length}>
                  <path d={d()} fill="transparent" stroke="none" />
                </Show>
              </>
            )}
          </Show>

          <Show when={isContainer()}>
            <For each={node().children ?? []}>{(child) => <SceneNode node={child} />}</For>
          </Show>
        </g>
      </g>
    </Show>
  );
}
