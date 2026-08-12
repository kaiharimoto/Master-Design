/**
 * The layers tree.
 *
 * Also the only place roles are visible at a glance, which matters more than it looks:
 * roles are what animations and AI instructions address, so a document whose roles are
 * invisible is one where "stagger the cards" quietly does nothing.
 */

import { For, Show } from "solid-js";
import { actions, index, isSelected, page, state } from "../store";
import type { Node } from "../types";

const ICONS: Record<string, string> = {
  frame: "▣",
  group: "◈",
  path: "✎",
  rect: "▭",
  ellipse: "◯",
  text: "T",
  image: "▨",
};

export function Layers() {
  const root = () => page()?.root ?? null;

  return (
    <div class="panel panel--layers">
      <header class="panel__header">
        <h2>Layers</h2>
        <Show when={state.selection.length > 1}>
          <span class="panel__badge">{state.selection.length} selected</span>
        </Show>
      </header>

      <div class="panel__body" role="tree" aria-label="Layers">
        <Show when={root()} fallback={<p class="panel__empty">No page open.</p>}>
          {(r) => (
            <For each={[...(r().children ?? [])].reverse()}>
              {(child) => <Row node={child} depth={0} />}
            </For>
          )}
        </Show>
      </div>
    </div>
  );
}

function Row(props: { node: Node; depth: number }) {
  const node = () => props.node;
  const hasChildren = () => (node().children?.length ?? 0) > 0;

  const toggle = (path: string, value: unknown, label: string) => {
    void actions.patch([{ op: "node.update", id: node().id, path, value }], label);
  };

  return (
    <>
      <div
        class="layer"
        classList={{
          "layer--selected": isSelected(node().id),
          "layer--hidden": node().visible === false,
        }}
        style={{ "padding-left": `${8 + props.depth * 14}px` }}
        role="treeitem"
        aria-selected={isSelected(node().id)}
        tabindex="0"
        onClick={(e) => {
          if (e.shiftKey || e.metaKey) actions.toggleSelect(node().id);
          else actions.select([node().id]);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            actions.select([node().id]);
          }
        }}
      >
        <span class="layer__icon" aria-hidden="true">
          {ICONS[node().type] ?? "•"}
        </span>
        <span class="layer__name">{node().name || node().type}</span>

        <For each={node().roles ?? []}>{(role) => <span class="layer__role">@{role}</span>}</For>

        <button
          class="layer__toggle"
          title={node().visible === false ? "Show" : "Hide"}
          aria-label={node().visible === false ? "Show layer" : "Hide layer"}
          onClick={(e) => {
            e.stopPropagation();
            toggle("visible", node().visible === false, "Toggle visibility");
          }}
        >
          {node().visible === false ? "◌" : "◉"}
        </button>
        <button
          class="layer__toggle"
          title={node().locked ? "Unlock" : "Lock"}
          aria-label={node().locked ? "Unlock layer" : "Lock layer"}
          onClick={(e) => {
            e.stopPropagation();
            toggle("locked", !node().locked, "Toggle lock");
          }}
        >
          {node().locked ? "🔒" : "🔓"}
        </button>
      </div>

      <Show when={hasChildren()}>
        <For each={[...(node().children ?? [])].reverse()}>
          {(child) => <Row node={child} depth={props.depth + 1} />}
        </For>
      </Show>
    </>
  );
}

/** Page switcher, shown above the layers on every shell. */
export function Pages() {
  const doc = () => state.editor?.document ?? null;

  return (
    <Show when={doc()}>
      {(d) => (
        <div class="pages" role="tablist" aria-label="Pages">
          <For each={d().pages}>
            {(p, i) => (
              <button
                role="tab"
                class="pages__tab"
                classList={{ "pages__tab--active": i() === state.pageIndex }}
                aria-selected={i() === state.pageIndex}
                onClick={() => actions.setPage(i())}
              >
                {p.name}
              </button>
            )}
          </For>
        </div>
      )}
    </Show>
  );
}

/** Ancestry of the current selection, so it is obvious what a click selected. */
export function Breadcrumb() {
  const trail = () => {
    const id = state.selection[0];
    if (!id) return [];
    const idx = index();
    const out: Node[] = [];
    let current: string | null = id;
    while (current) {
      const entry = idx.get(current);
      if (!entry) break;
      out.unshift(entry.node);
      current = entry.parent;
    }
    return out;
  };

  return (
    <Show when={trail().length > 0}>
      <nav class="breadcrumb" aria-label="Selection path">
        <For each={trail()}>
          {(node, i) => (
            <>
              <Show when={i() > 0}>
                <span class="breadcrumb__sep" aria-hidden="true">
                  ›
                </span>
              </Show>
              <button class="breadcrumb__item" onClick={() => actions.select([node.id])}>
                {node.name || node.type}
              </button>
            </>
          )}
        </For>
      </nav>
    </Show>
  );
}
