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

import { createResource, For, Show } from "solid-js";
import * as ipc from "../ipc";
import { fmt, toSvg } from "../math";
import { state } from "../store";
import type { Effect, GradientStop, Node, Paint, Stroke } from "../types";

/**
 * The URL for an asset, resolved through the backend.
 *
 * The exporter can write `href="assets/hero.png"` because the page sits next to its
 * assets folder. The canvas cannot: it is served from a dev server or from the app
 * bundle, and neither of those is the project directory. So the backend is asked where
 * the file is, and Tauri's asset protocol turns that into a URL the webview may load.
 *
 * A resource rather than a signal, because the answer is asynchronous and the image
 * should appear when it arrives rather than the whole node waiting on it.
 */
function useAssetUrl(name: () => string | undefined) {
  const [url] = createResource(name, async (asset) => {
    if (!asset) return undefined;
    // A name the backend refuses — one that would climb out of the project — resolves to
    // nothing, and the element renders empty rather than throwing away the whole canvas.
    return ipc.assets
      .path(asset)
      .then(ipc.assetUrl)
      .catch(() => undefined);
  });
  return url;
}

/** An `<image>` whose href comes from the project's assets folder. */
function AssetImage(props: {
  asset: string;
  width?: number;
  height?: number;
  fit?: string;
  objectBoundingBox?: boolean;
}) {
  const url = useAssetUrl(() => props.asset);
  return (
    <Show when={url()}>
      {(href) => (
        <image
          href={href()}
          width={props.objectBoundingBox ? 1 : props.width}
          height={props.objectBoundingBox ? 1 : props.height}
          preserveAspectRatio={
            props.fit === "contain"
              ? "xMidYMid meet"
              : props.fit === "fill"
                ? "none"
                : "xMidYMid slice"
          }
        />
      )}
    </Show>
  );
}

/** Stable, collision-free id for a gradient or filter belonging to one node. */
function defId(nodeId: string, kind: string, i: number) {
  return `md-${kind}-${nodeId}-${i}`;
}

export function paintValue(paint: Paint | undefined, nodeId: string, i: number): string {
  if (!paint) return "none";
  switch (paint.type) {
    case "solid":
      return colorHex(paint.color);
    case "linearGradient":
      return `url(#${defId(nodeId, "lg", i)})`;
    case "radialGradient":
      return `url(#${defId(nodeId, "rg", i)})`;
    case "image":
      return `url(#${defId(nodeId, "pat", i)})`;
  }
}

/** The `#rrggbb` half of a colour — all any SVG colour attribute will accept. */
function colorHex(color: string): string {
  return color.slice(0, 7);
}

/** A colour's alpha, which has to travel separately in an opacity attribute. */
function colorAlpha(color: string): number {
  return color.length === 9 ? parseInt(color.slice(7, 9), 16) / 255 : 1;
}

function paintOpacity(paint: Paint | undefined): number {
  if (!paint) return 1;
  const base = paint.opacity ?? 1;
  // A hex with alpha carries its own transparency; multiply the two rather than
  // letting one silently win. Gradient stops carry theirs in `stop-opacity`, so only
  // the paint's own opacity applies there.
  if (paint.type === "solid") return base * colorAlpha(paint.color);
  return base;
}

function PaintDefs(props: { node: Node }) {
  const paints = () => [
    ...(props.node.fills ?? []).map((p, i) => ({ paint: p, i })),
    ...(props.node.strokes ?? []).map((s, i) => ({ paint: s.paint, i: 1000 + i })),
  ];

  return (
    <For each={paints()}>
      {(entry) => {
        // Switching on the paint in plain code, rather than testing its tag inside a
        // `when`, is what lets each branch see the one variant it is written for. The
        // union narrows, so a paint that gains or renames a field fails to compile here
        // instead of quietly rendering as the wrong kind.
        const paint = entry.paint;
        switch (paint.type) {
          case "solid":
            return null;
          case "linearGradient":
            return (
              <linearGradient
                id={defId(props.node.id, "lg", entry.i)}
                x1={paint.from[0]}
                y1={paint.from[1]}
                x2={paint.to[0]}
                y2={paint.to[1]}
              >
                <Stops stops={paint.stops} />
              </linearGradient>
            );
          case "radialGradient":
            return (
              <radialGradient
                id={defId(props.node.id, "rg", entry.i)}
                cx={paint.center[0]}
                cy={paint.center[1]}
                r={paint.radius}
              >
                <Stops stops={paint.stops} />
              </radialGradient>
            );
          case "image":
            return (
              <pattern
                id={defId(props.node.id, "pat", entry.i)}
                patternContentUnits="objectBoundingBox"
                width={1}
                height={1}
              >
                <AssetImage asset={paint.asset} objectBoundingBox />
              </pattern>
            );
        }
      }}
    </For>
  );
}

function Stops(props: { stops: GradientStop[] }) {
  return (
    <For each={props.stops}>
      {(stop) => (
        <stop
          offset={stop.offset}
          stop-color={colorHex(stop.color)}
          stop-opacity={colorAlpha(stop.color)}
        />
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
        <For each={effects()}>{(effect, i) => effectPrimitives(effect, i())}</For>
      </filter>
    </Show>
  );
}

/**
 * The filter primitives one effect turns into.
 *
 * An effect with no branch here would leave the filter empty, and an empty filter's
 * output is transparent black — the node would vanish rather than merely lose its
 * shadow, so every variant has to be handled.
 */
function effectPrimitives(effect: Effect, index: number) {
  switch (effect.type) {
    case "blur":
      // SVG's stdDeviation is roughly half a CSS blur radius.
      return <feGaussianBlur stdDeviation={effect.radius / 2} />;
    case "dropShadow":
      return (
        <feDropShadow
          dx={effect.dx}
          dy={effect.dy}
          stdDeviation={effect.blur / 2}
          flood-color={colorHex(effect.color)}
          flood-opacity={colorAlpha(effect.color)}
        />
      );
    case "innerShadow": {
      // SVG has no inner-shadow primitive, so it is assembled: offset and blur the
      // source's alpha, subtract that from the alpha to leave the band just inside the
      // edge, and flood the band with the colour. Every step names its input and its
      // output, because implicit chaining reads as if the primitives compose in order
      // when they do not.
      const k = `inner${index}`;
      return (
        <>
          <feOffset in="SourceAlpha" dx={effect.dx} dy={effect.dy} result={`${k}-offset`} />
          <feGaussianBlur in={`${k}-offset`} stdDeviation={effect.blur / 2} result={`${k}-blur`} />
          <feComposite in="SourceAlpha" in2={`${k}-blur`} operator="out" result={`${k}-band`} />
          <feFlood
            flood-color={colorHex(effect.color)}
            flood-opacity={colorAlpha(effect.color)}
            result={`${k}-colour`}
          />
          <feComposite in={`${k}-colour`} in2={`${k}-band`} operator="in" result={`${k}-shadow`} />
          <feComposite in={`${k}-shadow`} in2="SourceGraphic" operator="over" />
        </>
      );
    }
  }
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
  // Unfilled text would be invisible, which is never what was meant, so it falls back
  // to black exactly as the exporter does.
  const fill = () => props.node.fills?.[0];

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
      text-decoration={
        props.node.decoration === "underline"
          ? "underline"
          : props.node.decoration === "strikethrough"
            ? "line-through"
            : undefined
      }
      fill={fill() ? paintValue(fill(), props.node.id, 0) : "#000000"}
      fill-opacity={paintOpacity(fill())}
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
  const roles = () => node().roles ?? [];

  // A clipping frame's own outline. Clipping to an empty region would delete the
  // children as well, so a frame with no area keeps them rather than compounding the
  // loss — the same bargain the exporter strikes.
  const clipShape = () => (node().type === "frame" && node().clip ? path() : null);
  const clipId = () => defId(node().id, "clip", 0);

  return (
    <Show when={node().visible !== false}>
      <g
        data-node-id={node().id}
        data-md-id={node().id}
        data-md-name={node().name || undefined}
        data-md-role={roles()[0]}
        class={roles().length ? roles().join(" ") : undefined}
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
        <Show when={clipShape()}>
          {(d) => (
            <clipPath id={clipId()}>
              <path d={d()} />
            </clipPath>
          )}
        </Show>

        <g transform={live() ? toSvg(live()!) : undefined}>
          <Show when={node().type === "text"}>
            <TextNode node={node()} />
          </Show>

          <Show when={node().type === "image" && node().asset}>
            {(asset) => (
              <AssetImage
                asset={asset()}
                width={node().width}
                height={node().height}
                fit={node().fit}
              />
            )}
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
            <g clip-path={clipShape() ? `url(#${clipId()})` : undefined}>
              <For each={node().children ?? []}>{(child) => <SceneNode node={child} />}</For>
            </g>
          </Show>
        </g>
      </g>
    </Show>
  );
}
