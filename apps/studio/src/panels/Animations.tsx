/**
 * The animation panel.
 *
 * This is where the package format earns itself. Nothing below knows what
 * `stagger-fade-up` is or what parameters it has — the controls are generated from the
 * manifest's `params`, so a new animation dropped into a folder arrives with a working
 * inspector and no code was written for it. That is the whole mechanism behind "keep
 * adding animations forever".
 */

import { createEffect, createResource, createSignal, For, on, Show } from "solid-js";
import * as ipc from "../ipc";
import { fmt } from "../math";
import { actions, page, state } from "../store";
import type { AnimationPackage, ParamSpec, Timeline } from "../types";
import { Field, Num, Section, Slider } from "./Inspector";

export function Animations() {
  const [packages] = createResource(() => ipc.anim.list());
  const [query, setQuery] = createSignal("");
  const [pending, setPending] = createSignal<string | null>(null);

  const timelines = () => page()?.timelines ?? [];

  const visible = () => {
    const all = packages() ?? [];
    const q = query().trim().toLowerCase();
    if (!q) return all;
    return all.filter((p) =>
      [p.manifest.id, p.manifest.title, p.manifest.description ?? "", ...(p.manifest.tags ?? [])]
        .join(" ")
        .toLowerCase()
        .includes(q),
    );
  };

  /** Selector the picker will apply to: a shared role if there is one, else the ids. */
  const targetSelector = () => {
    const nodes = state.selection
      .map((id) => page()?.root && findNode(page()!.root, id))
      .filter(Boolean);
    if (nodes.length === 0) return null;

    const shared = (nodes[0] as any)?.roles?.find((role: string) =>
      nodes.every((n: any) => n?.roles?.includes(role)),
    );
    // A role selector is worth far more than a list of ids: the timeline records it, so
    // it picks up nodes added later without anyone reapplying anything.
    if (shared) return `@${shared}`;
    return state.selection.map((id) => `#${id}`).join(", ");
  };

  async function apply(pkg: AnimationPackage) {
    const selector = targetSelector();
    const p = page();
    if (!selector || !p) return;

    setPending(pkg.manifest.id);
    try {
      const timeline = await ipc.anim.preview(pkg.manifest.id, selector, {}, p.slug);
      await actions.patch(
        [{ op: "timeline.set", page: p.slug, timeline }],
        `Apply ${pkg.manifest.title}`,
      );
    } finally {
      setPending(null);
    }
  }

  return (
    <div class="panel panel--animations">
      <header class="panel__header">
        <h2>Motion</h2>
      </header>

      <div class="panel__body">
        <Show when={timelines().length > 0}>
          <Section title="On this page">
            <For each={timelines()}>{(t) => <TimelineRow timeline={t} />}</For>
          </Section>
        </Show>

        <Section title="Library">
          <input
            type="search"
            placeholder="Search animations"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
          />

          <Show
            when={targetSelector()}
            fallback={<p class="panel__hint">Select something to animate it.</p>}
          >
            {(selector) => (
              <p class="panel__hint">
                Will apply to <code>{selector()}</code>
              </p>
            )}
          </Show>

          <Show when={packages.loading}>
            <p class="panel__hint">Loading…</p>
          </Show>

          <div class="anim-grid">
            <For each={visible()}>
              {(pkg) => (
                <button
                  class="anim-card"
                  disabled={!targetSelector() || pending() === pkg.manifest.id}
                  onClick={() => void apply(pkg)}
                  title={pkg.manifest.description}
                >
                  <span class="anim-card__title">{pkg.manifest.title}</span>
                  <span class="anim-card__meta">
                    {pkg.manifest.category}
                    <Show when={pkg.origin !== "standard"}> · {pkg.origin}</Show>
                  </span>
                  <span class="anim-card__desc">{pkg.manifest.description}</span>
                </button>
              )}
            </For>
          </div>

          <Show when={packages() && visible().length === 0}>
            <p class="panel__empty">Nothing matched “{query()}”.</p>
          </Show>
        </Section>
      </div>
    </div>
  );
}

function findNode(node: any, id: string): any {
  if (node.id === id) return node;
  for (const child of node.children ?? []) {
    const hit = findNode(child, id);
    if (hit) return hit;
  }
  return null;
}

// ---------------------------------------------------------------------------
// An applied timeline, with its parameters
// ---------------------------------------------------------------------------

function TimelineRow(props: { timeline: Timeline }) {
  const [expanded, setExpanded] = createSignal(false);
  const [packages] = createResource(() => ipc.anim.list());

  const manifest = () =>
    packages()?.find((p) => p.manifest.id === props.timeline.source?.package)?.manifest ?? null;

  const isScrubbed = () => state.playhead.timelineId === props.timeline.id;

  const setParam = async (key: string, value: unknown) => {
    const p = page();
    const source = props.timeline.source;
    if (!p || !source) return;

    // Re-bake rather than edit keyframes: the parameters are the truth, the keyframes
    // are a derived artifact. Editing the artifact would desynchronize the two.
    const rebaked = await ipc.anim.rebake(props.timeline.id, p.slug, {
      ...source.params,
      [key]: value,
    });
    await actions.patch(
      [{ op: "timeline.set", page: p.slug, timeline: rebaked }],
      `Change ${key}`,
    );
  };

  const remove = async () => {
    const p = page();
    if (!p) return;
    await actions.patch(
      [{ op: "timeline.remove", page: p.slug, id: props.timeline.id }],
      "Remove animation",
    );
  };

  return (
    <div class="timeline" classList={{ "timeline--open": expanded() }}>
      <div class="timeline__head">
        <button class="timeline__name" onClick={() => setExpanded(!expanded())}>
          {props.timeline.name || "Animation"}
        </button>
        <span class="timeline__meta">
          {props.timeline.trigger.type} · {fmt(props.timeline.duration)}s
        </span>
        <button
          class="mini"
          aria-label={props.timeline.enabled === false ? "Enable" : "Disable"}
          onClick={() =>
            void actions.patch(
              [
                {
                  op: "timeline.set",
                  page: page()!.slug,
                  timeline: { ...props.timeline, enabled: props.timeline.enabled === false },
                },
              ],
              "Toggle animation",
            )
          }
        >
          {props.timeline.enabled === false ? "◌" : "◉"}
        </button>
        <button class="mini" aria-label="Remove animation" onClick={() => void remove()}>
          ×
        </button>
      </div>

      <Show when={expanded()}>
        <Scrubber timeline={props.timeline} active={isScrubbed()} />

        <Show when={props.timeline.source}>
          {(source) => (
            <p class="panel__hint">
              {source().package} v{source().version} on <code>{source().target}</code>
            </p>
          )}
        </Show>

        <Show when={manifest()?.params}>
          {(params) => (
            <For each={params()}>
              {(spec) => (
                <ParamControl
                  spec={spec}
                  value={props.timeline.source?.params?.[spec.key] ?? spec.default}
                  onCommit={(v) => void setParam(spec.key, v)}
                />
              )}
            </For>
          )}
        </Show>

        <Show when={!props.timeline.source}>
          <p class="panel__hint">
            Hand-authored — no package to re-bake from, so parameters are not editable
            here.
          </p>
        </Show>
      </Show>
    </div>
  );
}

/**
 * Scrub the playhead.
 *
 * The canvas plays back by sampling the baked tracks, which is the same data the
 * exporter compiles — so what a scrub shows is what the built page will do.
 */
function Scrubber(props: { timeline: Timeline; active: boolean }) {
  const [time, setTime] = createSignal(0);

  createEffect(
    on(
      () => props.active,
      (active) => {
        if (!active) setTime(0);
      },
    ),
  );

  return (
    <div class="scrubber">
      <button
        class="mini"
        aria-label={state.playhead.playing ? "Pause" : "Play"}
        onClick={() => actions.setPlaying(!state.playhead.playing)}
      >
        {state.playhead.playing ? "❚❚" : "▶"}
      </button>
      <input
        type="range"
        min={0}
        max={props.timeline.duration}
        step={props.timeline.duration / 200}
        value={time()}
        onInput={(e) => {
          const t = Number(e.currentTarget.value);
          setTime(t);
          actions.setPlayhead(props.timeline.id, t);
        }}
      />
      <span class="scrubber__time">{fmt(time())}s</span>
    </div>
  );
}

/**
 * One control, generated from one parameter spec.
 *
 * The switch below is the entire cost of supporting a new animation: none, unless it
 * needs a parameter type that does not exist yet.
 */
function ParamControl(props: {
  spec: ParamSpec;
  value: unknown;
  onCommit: (value: unknown) => void;
}) {
  const spec = () => props.spec;

  return (
    <Field label={spec().label}>
      <Show when={spec().type === "number"}>
        <Show
          when={(spec() as any).min !== undefined && (spec() as any).max !== undefined}
          fallback={
            <Num
              value={Number(props.value ?? 0)}
              step={(spec() as any).step}
              suffix={(spec() as any).unit}
              onCommit={props.onCommit}
            />
          }
        >
          <Slider
            value={Number(props.value ?? 0)}
            min={(spec() as any).min}
            max={(spec() as any).max}
            step={(spec() as any).step ?? 0.01}
            onCommit={props.onCommit}
          />
        </Show>
      </Show>

      <Show when={spec().type === "color"}>
        <input
          type="color"
          value={String(props.value ?? "#000000").slice(0, 7)}
          onChange={(e) => props.onCommit(e.currentTarget.value)}
        />
      </Show>

      <Show when={spec().type === "boolean"}>
        <input
          type="checkbox"
          checked={Boolean(props.value)}
          onChange={(e) => props.onCommit(e.currentTarget.checked)}
        />
      </Show>

      <Show when={spec().type === "select"}>
        <select value={String(props.value ?? "")} onChange={(e) => props.onCommit(e.currentTarget.value)}>
          <For each={(spec() as any).options}>
            {(option: { value: string; label: string }) => (
              <option value={option.value}>{option.label}</option>
            )}
          </For>
        </select>
      </Show>

      <Show when={spec().type === "easing"}>
        <select
          value={typeof props.value === "string" ? props.value : "custom"}
          onChange={(e) => props.onCommit(e.currentTarget.value)}
        >
          <For each={["linear", "easeIn", "easeOut", "easeInOut"]}>
            {(name) => <option value={name}>{name}</option>}
          </For>
          <Show when={typeof props.value !== "string"}>
            <option value="custom">custom curve</option>
          </Show>
        </select>
      </Show>

      <Show when={spec().type === "text"}>
        <input
          type="text"
          value={String(props.value ?? "")}
          onChange={(e) => props.onCommit(e.currentTarget.value)}
        />
      </Show>
    </Field>
  );
}
