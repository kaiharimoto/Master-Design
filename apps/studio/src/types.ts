/**
 * The document shapes, mirroring `md-doc`'s JSON.
 *
 * Hand-written rather than generated. Generation would guarantee they stay in step,
 * but it also puts a codegen step between editing a Rust struct and seeing the app
 * compile — and the backend rejects anything malformed anyway, so a drift here surfaces
 * as a clear error from the patch protocol rather than as corrupt data. When the model
 * settles, this is the obvious thing to generate.
 */

export type NodeId = string;
export type PageId = string;
export type TimelineId = string;

/** `[a, b, c, d, e, f]` — the same six numbers as SVG's `matrix()`. */
export type Matrix = [number, number, number, number, number, number];

export type NodeType = "frame" | "group" | "path" | "rect" | "ellipse" | "text" | "image";

export interface Color {
  /** `#rrggbb` or `#rrggbbaa`. */
  hex: string;
}

export type Paint =
  | { type: "solid"; color: string; opacity?: number }
  | {
      type: "linearGradient";
      from: [number, number];
      to: [number, number];
      stops: GradientStop[];
      opacity?: number;
    }
  | {
      type: "radialGradient";
      center: [number, number];
      radius: number;
      stops: GradientStop[];
      opacity?: number;
    }
  | { type: "image"; asset: string; fit?: ImageFit; opacity?: number };

export interface GradientStop {
  offset: number;
  color: string;
}

export type ImageFit = "cover" | "contain" | "fill" | "tile";
export type LineCap = "butt" | "round" | "square";
export type LineJoin = "miter" | "round" | "bevel";
export type StrokeAlign = "center" | "inside" | "outside";

export interface Stroke {
  paint: Paint;
  width: number;
  cap?: LineCap;
  join?: LineJoin;
  miterLimit?: number;
  dash?: number[];
  dashOffset?: number;
  align?: StrokeAlign;
}

export type Effect =
  | { type: "blur"; radius: number }
  | { type: "dropShadow"; dx: number; dy: number; blur: number; color: string }
  | { type: "innerShadow"; dx: number; dy: number; blur: number; color: string };

export interface A11y {
  role?: string;
  label?: string;
  headingLevel?: number;
  alt?: string;
  hidden?: boolean;
}

export interface Layout {
  direction: "vertical" | "horizontal";
  gap?: number;
  padding?: [number, number, number, number];
  align?: "start" | "center" | "end" | "stretch";
  justify?: "start" | "center" | "end" | "spaceBetween";
  wrap?: boolean;
}

/**
 * One node. Kind-specific fields are flattened alongside the common ones, exactly as
 * they appear on disk, so a node can be handed to `node.update` without reshaping.
 */
export interface Node {
  id: NodeId;
  type: NodeType;
  name?: string;
  transform?: Matrix;
  visible?: boolean;
  locked?: boolean;
  opacity?: number;
  blendMode?: string;
  fills?: Paint[];
  strokes?: Stroke[];
  effects?: Effect[];
  roles?: string[];
  a11y?: A11y;
  children?: Node[];

  // frame, rect, ellipse, image
  width?: number;
  height?: number;
  cornerRadius?: [number, number, number, number];
  clip?: boolean;
  layout?: Layout;

  // path
  d?: string;
  fillRule?: "nonZero" | "evenOdd";

  // image
  asset?: string;
  fit?: ImageFit;

  // text
  content?: string;
  fontFamily?: string;
  fontSize?: number;
  fontWeight?: number;
  italic?: boolean;
  letterSpacing?: number;
  lineHeight?: number;
  align?: "left" | "center" | "right" | "justify";
  textCase?: "original" | "upper" | "lower" | "title";
  decoration?: "none" | "underline" | "strikethrough";
}

export type Easing =
  | "linear"
  | "easeIn"
  | "easeOut"
  | "easeInOut"
  | { cubicBezier: [number, number, number, number] }
  | { steps: number };

export interface Keyframe {
  t: number;
  value: unknown;
  easing?: Easing;
}

export interface Track {
  target: NodeId;
  property: string;
  keyframes: Keyframe[];
}

export type Trigger =
  | { type: "load"; delay?: number }
  | { type: "view"; threshold?: number; once?: boolean }
  | { type: "scroll"; start?: number; end?: number }
  | { type: "hover" }
  | { type: "click" }
  | { type: "loop"; iterations?: number; alternate?: boolean };

export interface AnimSource {
  package: string;
  version: string;
  target: string;
  params: Record<string, unknown>;
}

export interface Timeline {
  id: TimelineId;
  name?: string;
  trigger: Trigger;
  duration: number;
  enabled?: boolean;
  reducedMotion?: "skip" | "play" | "ignore";
  source?: AnimSource;
  tracks: Track[];
}

export interface Page {
  id: PageId;
  name: string;
  slug: string;
  width: number;
  height: number;
  background?: Paint;
  root: Node;
  timelines?: Timeline[];
}

export interface Document {
  schemaVersion: number;
  meta: { name: string; description?: string; breakpoints?: { name: string; minWidth: number }[] };
  tokens?: {
    colors?: Record<string, string>;
    fonts?: Record<string, { family: string; size: number; weight?: number }>;
    spacing?: Record<string, number>;
  };
  pages: Page[];
}

// ---------------------------------------------------------------------------
// Operations — the only way anything changes
// ---------------------------------------------------------------------------

export type Op =
  | { op: "node.insert"; parent: NodeId; index?: number; node: Node }
  | { op: "node.delete"; id: NodeId }
  | { op: "node.update"; id: NodeId; path: string; value: unknown }
  | { op: "node.move"; id: NodeId; parent: NodeId; index?: number }
  | { op: "path.boolean"; ids: NodeId[]; mode: BoolMode; resultId?: NodeId }
  | { op: "page.insert"; page: Page; index?: number }
  | { op: "page.delete"; page: string }
  | { op: "page.update"; page: string; path: string; value: unknown }
  | { op: "timeline.set"; page: string; timeline: Timeline }
  | { op: "timeline.remove"; page: string; id: TimelineId }
  | { op: "tokens.set"; path: string; value: unknown };

export type BoolMode = "union" | "subtract" | "intersect" | "exclude";

export interface PatchReport {
  applied: number;
  touched: NodeId[];
  warnings: string[];
}

// ---------------------------------------------------------------------------
// Animation packages
// ---------------------------------------------------------------------------

export type ParamKind =
  | { type: "number"; min?: number; max?: number; step?: number; unit?: string }
  | { type: "color" }
  | { type: "boolean" }
  | { type: "select"; options: { value: string; label: string }[] }
  | { type: "easing" }
  | { type: "text" };

export type ParamSpec = ParamKind & {
  key: string;
  label: string;
  default: unknown;
  description?: string;
};

export interface AnimationManifest {
  id: string;
  version: string;
  title: string;
  description?: string;
  category: "entrance" | "exit" | "emphasis" | "ambient" | "scroll" | "interaction";
  tags?: string[];
  appliesTo?: { minTargets?: number; maxTargets?: number; kinds?: string[] };
  defaultTrigger: Trigger;
  params?: ParamSpec[];
}

export interface AnimationPackage {
  manifest: AnimationManifest;
  origin: "standard" | "user" | "project";
}

// ---------------------------------------------------------------------------
// Editor state exchanged with the backend
// ---------------------------------------------------------------------------

export interface EditorState {
  document: Document;
  projectPath: string | null;
  revision: number;
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  dirty: boolean;
}

export interface UpdateInfo {
  available: boolean;
  currentVersion: string;
  latestVersion?: string;
  notes?: string;
  releaseUrl?: string;
  /** Direct download for the current platform's artifact, when there is one. */
  downloadUrl?: string;
  /** Hex SHA-256 of that artifact, published in the release manifest. */
  sha256?: string;
}
