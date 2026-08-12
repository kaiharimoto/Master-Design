//! Tool definitions.
//!
//! These descriptions are read by a model, not by a person, and they are the entire
//! briefing it gets. Each one says what the tool is *for* and when to reach for it,
//! because a model choosing between `doc_query` and `doc_describe` has nothing else to
//! go on.
//!
//! Names use underscores rather than the dots the operation protocol uses: some MCP
//! clients restrict tool names to `[A-Za-z0-9_-]`, and a tool that cannot be called is
//! worse than one with a slightly inconsistent name.

use crate::protocol::{schema, ToolSpec};
use serde_json::json;

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "doc_describe",
            description:
                "Read the whole design as a compact outline: pages, the node tree with ids, \
                 roles, sizes and fills, and the timelines on each page. Start here — it is \
                 far smaller than the project JSON and tells you the role vocabulary you can \
                 address with selectors.",
            input_schema: schema(
                json!({
                    "page": { "type": "string", "description": "Limit to one page, by slug or id." },
                    "maxDepth": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "How deep to descend before summarizing. Default 12."
                    }
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "doc_query",
            description:
                "Find nodes with a selector and get their full properties. Use this when you \
                 need exact values rather than the overview doc_describe gives. Selector \
                 syntax: '#nd_abc' by id, '@card' by role, 'type:text' by kind, \
                 'name:\"Hero\"' by layer name, '*' for everything, space for descendant, \
                 comma for either.",
            input_schema: schema(
                json!({
                    "selector": { "type": "string", "description": "e.g. '@card' or 'type:path'." },
                    "page": { "type": "string", "description": "Limit to one page." },
                    "properties": {
                        "type": "boolean",
                        "description": "Include each node's full JSON. Default true. Set false \
                                        for just the matching ids."
                    }
                }),
                &["selector"],
            ),
        },
        ToolSpec {
            name: "doc_patch",
            description:
                "Change the design. Every edit goes through this, and the whole list applies \
                 as one undoable step — if any operation fails, nothing is applied. \
                 Operations: node.insert, node.delete, node.update, node.move, path.boolean, \
                 page.insert, page.delete, page.update, timeline.set, timeline.remove, \
                 tokens.set. node.update takes a dotted path such as 'width' or \
                 'fills.0.color', and a null value clears an optional property.",
            input_schema: schema(
                json!({
                    "ops": {
                        "type": "array",
                        "description": "Operations, each an object with an 'op' field.",
                        "items": { "type": "object" }
                    },
                    "label": {
                        "type": "string",
                        "description": "What to show in the undo menu, e.g. 'Add hero section'. \
                                        The person editing alongside you will read this."
                    }
                }),
                &["ops"],
            ),
        },
        ToolSpec {
            name: "doc_snapshot",
            description:
                "Render the design to a PNG and look at it. Use this after making changes — \
                 a node list cannot tell you that two elements overlap, that text is \
                 illegible against its background, or that something sits off-canvas. Pass \
                 'time' to render mid-animation, which is the only way to see whether motion \
                 you added actually reads.",
            input_schema: schema(
                json!({
                    "page": { "type": "string", "description": "Page slug or id. Defaults to the first page." },
                    "node": { "type": "string", "description": "Crop to this node, with padding." },
                    "region": {
                        "type": "array",
                        "items": { "type": "number" },
                        "description": "Crop rectangle in document units: [x, y, width, height]."
                    },
                    "time": {
                        "type": "number",
                        "description": "Seconds into the page's animations. Omit for the resting \
                                        state. For scroll-linked timelines this is progress 0..1."
                    },
                    "width": {
                        "type": "integer",
                        "description": "Output width in pixels, 16..4096. Default 1024."
                    }
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "doc_undo",
            description:
                "Undo the last change, whoever made it — your patches and the person's mouse \
                 edits share one history.",
            input_schema: schema(json!({}), &[]),
        },
        ToolSpec {
            name: "doc_redo",
            description: "Reapply the last undone change. Note that any new edit — yours or the \
                 person's — discards the redo branch, so this only works if nothing has \
                 happened since the undo.",
            input_schema: schema(json!({}), &[]),
        },
        ToolSpec {
            name: "anim_list",
            description:
                "List the installed animation packages, with their parameters and defaults. \
                 Read this before anim_apply — parameter names and ranges are per package, \
                 and applying with a name the package does not define is refused.",
            input_schema: schema(
                json!({
                    "query": { "type": "string", "description": "Filter by name, description or tag." },
                    "category": {
                        "type": "string",
                        "enum": ["entrance", "exit", "emphasis", "ambient", "scroll", "interaction"]
                    }
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "anim_apply",
            description: "Apply an animation package to the nodes a selector matches, and add the \
                 resulting timeline to the page. Prefer a role selector like '@card' over a \
                 list of ids: the timeline records the selector, so it picks up nodes added \
                 later. Stagger follows document order.",
            input_schema: schema(
                json!({
                    "package": { "type": "string", "description": "Package id, e.g. 'std/stagger-fade-up'." },
                    "selector": { "type": "string", "description": "Which nodes to animate, e.g. '@card'." },
                    "params": {
                        "type": "object",
                        "description": "Parameter overrides. Anything omitted uses the package default."
                    },
                    "page": { "type": "string", "description": "Page to add the timeline to. Defaults to the first." },
                    "name": { "type": "string", "description": "Name for the timeline in the editor." }
                }),
                &["package", "selector"],
            ),
        },
        ToolSpec {
            name: "export_build",
            description:
                "Build the static site — HTML, CSS, inline SVG and the animation runtime — \
                 into a directory. Returns the files written and any warnings.",
            input_schema: schema(
                json!({
                    "out": { "type": "string", "description": "Output directory. Defaults to 'dist' inside the project." },
                    "page": { "type": "string", "description": "Export only this page." }
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "selection_get",
            description:
                "Read what the person currently has selected in the studio, along with any \
                 note they typed and any annotation they drew. This is how a request like \
                 'make this bounce' becomes actionable: it tells you which nodes 'this' \
                 means. Returns empty if the studio is not running.",
            input_schema: schema(json!({}), &[]),
        },
        ToolSpec {
            name: "requests_list",
            description:
                "Read queued notes the person left for you, oldest first. These come from \
                 sessions where no agent was attached — most often from the phone, where \
                 there is no terminal to run one in. Check this at the start of a session.",
            input_schema: schema(
                json!({
                    "includeResolved": { "type": "boolean", "description": "Include ones already dealt with. Default false." }
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "requests_resolve",
            description: "Mark a queued request as done or dismissed once you have acted on it.",
            input_schema: schema(
                json!({
                    "id": { "type": "string" },
                    "status": { "type": "string", "enum": ["done", "dismissed"] }
                }),
                &["id"],
            ),
        },
    ]
}
