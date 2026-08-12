//! # md-mcp
//!
//! A Model Context Protocol server over a Master Design project.
//!
//! This is the bridge. An AI model attached here can read the design, change it, and —
//! importantly — *look at the result*. Every change it makes goes through the same
//! operation protocol a mouse drag goes through, lands on the same undo stack, and gets
//! the same validation, so nothing it does is a special case the person then has to
//! clean up.
//!
//! It runs two ways:
//!
//! * beside the studio, where `selection_get` tells it what the person is pointing at
//! * headless, over a project directory, where `requests_list` tells it what they asked
//!   for while no agent was attached
//!
//! The second case is what makes the phone useful. There is no terminal on a phone to
//! run an agent in, so a note written there is committed into the project and picked up
//! by a session on a desktop later.

pub mod protocol;
pub mod render;
pub mod tools;

use base64::Engine;
use md_anim::{Origin, Registry};
use md_doc::digest::{digest, DigestOptions};
use md_doc::history::Session;
use md_doc::patch::Op;
use md_doc::storage::{self, RequestStatus};
use md_doc::{Document, NodeId};
use protocol::{codes, Request, Response, ToolResult, PROTOCOL_VERSION};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub struct Server {
    project_dir: PathBuf,
    session: Session,
    registry: Registry,
    /// Write the project back to disk after every successful change.
    ///
    /// On by default: the studio watches these files, so this is what makes an AI edit
    /// appear on the person's canvas rather than sitting in a process they cannot see.
    pub autosave: bool,
}

impl Server {
    /// Open a project directory.
    pub fn open(
        project_dir: impl AsRef<Path>,
        standard_animations: Option<&Path>,
    ) -> Result<Self, String> {
        let dir = project_dir.as_ref().to_path_buf();
        let doc = storage::load_project(&dir).map_err(|e| e.to_string())?;

        let mut registry = Registry::new();
        if let Some(std_dir) = standard_animations {
            registry.load_dir(std_dir, Origin::Standard);
        }
        registry.load_dir(&dir.join(storage::ANIMATIONS_DIR), Origin::Project);

        Ok(Server {
            project_dir: dir,
            session: Session::new(doc),
            registry,
            autosave: true,
        })
    }

    /// Build a server over an in-memory document, for tests.
    pub fn with_document(project_dir: impl AsRef<Path>, doc: Document, registry: Registry) -> Self {
        Server {
            project_dir: project_dir.as_ref().to_path_buf(),
            session: Session::new(doc),
            registry,
            autosave: false,
        }
    }

    pub fn document(&self) -> &Document {
        self.session.document()
    }

    /// Read newline-delimited JSON-RPC from a reader, write responses to a writer.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => self.handle(req),
                Err(e) => Some(Response::err(
                    Value::Null,
                    codes::PARSE_ERROR,
                    format!("could not parse request: {e}"),
                )),
            };

            // Notifications get no reply, which is the protocol's way of saying "do not
            // block on this".
            if let Some(response) = response {
                writeln!(output, "{}", serde_json::to_string(&response)?)?;
                output.flush()?;
            }
        }
        Ok(())
    }

    pub fn serve_stdio(&mut self) -> std::io::Result<()> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        self.serve(stdin.lock(), stdout.lock())
    }

    pub fn handle(&mut self, req: Request) -> Option<Response> {
        let id = req.id.clone();

        let result = match req.method.as_str() {
            "initialize" => Ok(self.initialize(req.params.as_ref())),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::specs() })),
            "tools/call" => self.tools_call(req.params.as_ref()),
            // Notifications: acknowledged by doing nothing, as required.
            m if m.starts_with("notifications/") => return None,
            other => Err((codes::METHOD_NOT_FOUND, format!("unknown method '{other}'"))),
        };

        let id = id?;
        Some(match result {
            Ok(value) => Response::ok(id, value),
            Err((code, message)) => Response::err(id, code, message),
        })
    }

    fn initialize(&self, params: Option<&Value>) -> Value {
        // Echo the client's protocol revision when they name one; revisions so far have
        // been additive, and refusing a newer client we can still serve helps nobody.
        let version = params
            .and_then(|p| p.get("protocolVersion"))
            .and_then(|v| v.as_str())
            .unwrap_or(PROTOCOL_VERSION)
            .to_string();

        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "master-design",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": self.instructions(),
        })
    }

    fn instructions(&self) -> String {
        let doc = self.session.document();
        let roles: Vec<String> = doc.role_index().keys().map(|r| format!("@{r}")).collect();

        format!(
            "You are editing the Master Design project \"{}\" ({} page(s), {} nodes).\n\n\
             Start with doc_describe to see the structure. Change things with doc_patch — \
             never by writing files directly, since patches are validated, atomic and \
             undoable, and a person may be editing the same document at the same time. \
             After changing anything visual, call doc_snapshot and look at it.\n\n\
             Address nodes by role rather than by id where you can ({}). Roles survive \
             edits and let one animation apply to everything that should have it.\n\n\
             Check requests_list at the start of a session: it holds notes left for you \
             from sessions where no agent was attached.",
            doc.meta.name,
            doc.pages.len(),
            doc.node_count(),
            if roles.is_empty() {
                "this document has no roles yet — consider adding some".to_string()
            } else {
                roles.join(" ")
            }
        )
    }

    fn tools_call(&mut self, params: Option<&Value>) -> Result<Value, (i32, String)> {
        let params = params.ok_or((codes::INVALID_PARAMS, "missing params".to_string()))?;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or((codes::INVALID_PARAMS, "missing tool name".to_string()))?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match name {
            "doc_describe" => self.doc_describe(&args),
            "doc_query" => self.doc_query(&args),
            "doc_patch" => self.doc_patch(&args),
            "doc_snapshot" => self.doc_snapshot(&args),
            "doc_undo" => self.doc_undo(),
            "doc_redo" => self.doc_redo(),
            "anim_list" => self.anim_list(&args),
            "anim_apply" => self.anim_apply(&args),
            "export_build" => self.export_build(&args),
            "selection_get" => self.selection_get(),
            "requests_list" => self.requests_list(&args),
            "requests_resolve" => self.requests_resolve(&args),
            other => return Err((codes::METHOD_NOT_FOUND, format!("no tool named '{other}'"))),
        };

        Ok(result.to_value())
    }

    // -----------------------------------------------------------------------
    // Tools
    // -----------------------------------------------------------------------

    fn doc_describe(&self, args: &Value) -> ToolResult {
        let opts = DigestOptions {
            page: args.get("page").and_then(|v| v.as_str()).map(String::from),
            max_depth: args
                .get("maxDepth")
                .and_then(|v| v.as_u64())
                .map(|d| d as usize)
                .unwrap_or(12),
            ..Default::default()
        };
        ToolResult::text(digest(self.session.document(), &opts))
    }

    fn doc_query(&self, args: &Value) -> ToolResult {
        let selector = match args.get("selector").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("selector is required"),
        };

        let parsed = match md_doc::Selector::parse(selector) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let doc = self.session.document();
        let ids = match args.get("page").and_then(|v| v.as_str()) {
            Some(page) => match parsed.select_in_page(doc, page) {
                Ok(ids) => ids,
                Err(e) => return ToolResult::error(e.to_string()),
            },
            None => parsed.select(doc),
        };

        if ids.is_empty() {
            return ToolResult::text(format!(
                "'{selector}' matched nothing. Run doc_describe to see the roles and ids \
                 this document actually has."
            ));
        }

        let with_properties = args
            .get("properties")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if !with_properties {
            let list: Vec<&str> = ids.iter().map(|i| i.as_str()).collect();
            return ToolResult::text(format!("{} match(es): {}", ids.len(), list.join(", ")));
        }

        let nodes: Vec<Value> = ids
            .iter()
            .filter_map(|id| doc.node(id))
            .filter_map(|n| serde_json::to_value(n).ok())
            .collect();

        match md_doc::to_canonical_string(&json!({ "matches": ids.len(), "nodes": nodes })) {
            Ok(text) => ToolResult::text(text),
            Err(e) => ToolResult::error(e.to_string()),
        }
    }

    fn doc_patch(&mut self, args: &Value) -> ToolResult {
        let raw = match args.get("ops") {
            Some(v) => v.clone(),
            None => return ToolResult::error("ops is required"),
        };

        let ops: Vec<Op> = match serde_json::from_value(raw) {
            Ok(ops) => ops,
            Err(e) => {
                return ToolResult::error(format!(
                    "could not read the operations: {e}\n\n\
                     Each operation is an object with an 'op' field, e.g. \
                     {{\"op\":\"node.update\",\"id\":\"nd_abc\",\"path\":\"width\",\"value\":320}}"
                ))
            }
        };

        if ops.is_empty() {
            return ToolResult::error("no operations to apply");
        }

        let label = args
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("AI edit")
            .to_string();

        match self.session.apply(label.clone(), ops) {
            Ok(report) => {
                if let Err(e) = self.save() {
                    return ToolResult::error(format!(
                        "the change applied but could not be saved: {e}"
                    ));
                }
                let touched: Vec<&str> = report.touched.iter().map(|i| i.as_str()).collect();
                let mut text = format!(
                    "Applied {} operation(s) as \"{}\". Touched: {}",
                    report.applied,
                    label,
                    if touched.is_empty() {
                        "nothing".to_string()
                    } else {
                        touched.join(", ")
                    }
                );
                for w in &report.warnings {
                    text.push_str(&format!("\nWarning: {w}"));
                }
                text.push_str("\n\nCall doc_snapshot to see the result.");
                ToolResult::text(text)
            }
            // A refused patch changed nothing at all — say so, because the natural
            // assumption after an error is that some of it landed.
            Err(e) => ToolResult::error(format!(
                "{e}\n\nNothing was applied; the document is unchanged."
            )),
        }
    }

    fn doc_snapshot(&self, args: &Value) -> ToolResult {
        let doc = self.session.document();
        let page = args
            .get("page")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                doc.pages
                    .first()
                    .map(|p| p.slug.clone())
                    .unwrap_or_default()
            });

        let node = match args.get("node").and_then(|v| v.as_str()) {
            Some(s) => match NodeId::parse(s) {
                Ok(id) => Some(id),
                Err(e) => return ToolResult::error(e.to_string()),
            },
            None => None,
        };

        let region = args.get("region").and_then(|v| v.as_array()).and_then(|a| {
            let nums: Vec<f64> = a.iter().filter_map(|v| v.as_f64()).collect();
            if nums.len() == 4 {
                Some([nums[0], nums[1], nums[2], nums[3]])
            } else {
                None
            }
        });

        let opts = render::SnapshotOptions {
            width: args.get("width").and_then(|v| v.as_u64()).unwrap_or(1024) as u32,
            time: args.get("time").and_then(|v| v.as_f64()),
            node,
            region,
            at_width: args.get("atWidth").and_then(|v| v.as_f64()),
        };

        match render::snapshot(doc, &page, &opts) {
            Ok((png, w, h)) => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
                let when = match opts.time {
                    Some(t) => format!(" at {t}s"),
                    None => String::new(),
                };
                let laid_out = match opts.at_width {
                    Some(width) => format!(", laid out at {width} document units wide"),
                    None => String::new(),
                };
                ToolResult::image(
                    format!("Page \"{page}\"{when}{laid_out}, {w}×{h}."),
                    encoded,
                )
            }
            Err(e) => ToolResult::error(e),
        }
    }

    fn doc_undo(&mut self) -> ToolResult {
        let label = self.session.history().undo_label().map(String::from);
        match self.session.undo() {
            Ok(_) => {
                if let Err(e) = self.save() {
                    return ToolResult::error(format!("undone, but could not save: {e}"));
                }
                ToolResult::text(format!(
                    "Undid \"{}\".",
                    label.unwrap_or_else(|| "the last change".into())
                ))
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }

    fn doc_redo(&mut self) -> ToolResult {
        match self.session.redo() {
            Ok(_) => {
                if let Err(e) = self.save() {
                    return ToolResult::error(format!("redone, but could not save: {e}"));
                }
                ToolResult::text("Redone.")
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }

    fn anim_list(&self, args: &Value) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let mut packages = self.registry.search(query);

        if let Some(category) = args.get("category").and_then(|v| v.as_str()) {
            packages.retain(|p| p.manifest.category.as_str() == category);
        }

        if packages.is_empty() {
            return ToolResult::text("No animation packages matched.");
        }

        let mut out = String::new();
        for pkg in packages {
            let m = &pkg.manifest;
            out.push_str(&format!(
                "{} v{} — {}\n  {}\n  applies to: {}\n",
                m.id,
                m.version,
                m.title,
                m.description,
                md_anim::bake::kind_summary(m)
            ));
            for p in &m.params {
                let detail = match &p.kind {
                    md_anim::ParamKind::Number { min, max, unit, .. } => {
                        let range = match (min, max) {
                            (Some(lo), Some(hi)) => format!(" [{lo}..{hi}]"),
                            _ => String::new(),
                        };
                        format!(
                            "number{range}{}",
                            if unit.is_empty() {
                                String::new()
                            } else {
                                format!(" {unit}")
                            }
                        )
                    }
                    md_anim::ParamKind::Select { options } => format!(
                        "one of {}",
                        options
                            .iter()
                            .map(|o| o.value.as_str())
                            .collect::<Vec<_>>()
                            .join("|")
                    ),
                    other => format!("{other:?}").to_lowercase(),
                };
                out.push_str(&format!(
                    "    {} ({}) default {}{}\n",
                    p.key,
                    detail,
                    p.default,
                    if p.description.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", p.description)
                    }
                ));
            }
            out.push('\n');
        }
        ToolResult::text(out)
    }

    fn anim_apply(&mut self, args: &Value) -> ToolResult {
        let package = match args.get("package").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return ToolResult::error("package is required"),
        };
        let selector = match args.get("selector").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => return ToolResult::error("selector is required"),
        };

        let doc = self.session.document();
        let page_key = args
            .get("page")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                doc.pages
                    .first()
                    .map(|p| p.slug.clone())
                    .unwrap_or_default()
            });

        let params = args
            .get("params")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let mut timeline = match md_anim::apply_to_selector(
            doc,
            &self.registry,
            &package,
            &selector,
            params,
            Some(&page_key),
        ) {
            Ok(t) => t,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        if let Some(name) = args.get("name").and_then(|v| v.as_str()) {
            timeline.name = name.to_string();
        }

        let summary = format!(
            "Applied {} to '{}' on page '{}': {} track(s) over {}s, triggered on {}.",
            package,
            selector,
            page_key,
            timeline.tracks.len(),
            md_geom::fmt_coord(timeline.duration),
            timeline.trigger.type_name()
        );

        match self.session.apply(
            format!("Apply {package}"),
            vec![Op::TimelineSet {
                page: page_key,
                timeline: Box::new(timeline),
            }],
        ) {
            Ok(_) => {
                if let Err(e) = self.save() {
                    return ToolResult::error(format!("applied, but could not save: {e}"));
                }
                ToolResult::text(format!(
                    "{summary}\n\nCall doc_snapshot with a 'time' partway through to see it."
                ))
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }

    fn export_build(&self, args: &Value) -> ToolResult {
        let out = args
            .get("out")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.project_dir.join("dist"));

        let opts = md_emit::ExportOptions {
            assets_from: Some(self.project_dir.join(storage::ASSETS_DIR)),
            only_page: args.get("page").and_then(|v| v.as_str()).map(String::from),
            ..Default::default()
        };

        match md_emit::export(self.session.document(), &out, &opts) {
            Ok(report) => {
                let mut text = format!(
                    "Exported {} file(s), {} bytes, to {}",
                    report.files.len(),
                    report.bytes,
                    out.display()
                );
                for w in &report.warnings {
                    text.push_str(&format!("\nWarning: {w}"));
                }
                ToolResult::text(text)
            }
            Err(e) => ToolResult::error(e.to_string()),
        }
    }

    fn selection_get(&self) -> ToolResult {
        let selection = md_doc::selection::load_selection(&self.project_dir);
        if selection.is_empty() {
            return ToolResult::text(
                "Nothing is selected. The studio may not be running — check requests_list \
                 for notes left for you asynchronously.",
            );
        }

        let doc = self.session.document();
        let mut text = format!("Page: {}\n", selection.page);
        if !selection.note.is_empty() {
            text.push_str(&format!("They said: \"{}\"\n", selection.note));
        }
        text.push_str(&format!("Selected {} node(s):\n", selection.nodes.len()));
        for id in &selection.nodes {
            match doc.node(id) {
                Some(n) => text.push_str(&format!(
                    "  {} {} \"{}\"{}\n",
                    n.kind.type_name(),
                    n.id,
                    n.display_name(),
                    if n.roles.is_empty() {
                        String::new()
                    } else {
                        format!(" @{}", n.roles.join(" @"))
                    }
                )),
                None => text.push_str(&format!("  {id} (no longer in the document)\n")),
            }
        }
        if let Some(annotation) = &selection.annotation {
            text.push_str(&format!(
                "They drew on the canvas; the markup is at {annotation}\n"
            ));
        }
        if let Some(v) = selection.viewport {
            text.push_str(&format!(
                "They are looking at [{}, {}, {}, {}] — pass that as doc_snapshot's region \
                 to see what they see.\n",
                md_geom::fmt_coord(v[0]),
                md_geom::fmt_coord(v[1]),
                md_geom::fmt_coord(v[2]),
                md_geom::fmt_coord(v[3])
            ));
        }
        ToolResult::text(text)
    }

    fn requests_list(&self, args: &Value) -> ToolResult {
        let include_resolved = args
            .get("includeResolved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let requests = match storage::load_requests(&self.project_dir) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let visible: Vec<_> = requests
            .iter()
            .filter(|r| include_resolved || r.status == RequestStatus::Open)
            .collect();

        if visible.is_empty() {
            return ToolResult::text("No queued requests.");
        }

        let mut text = format!("{} request(s):\n", visible.len());
        for r in visible {
            text.push_str(&format!(
                "\n{} [{:?}] on page {}\n  \"{}\"\n",
                r.id, r.status, r.page, r.note
            ));
            if !r.selection.is_empty() {
                let ids: Vec<&str> = r.selection.iter().map(|i| i.as_str()).collect();
                text.push_str(&format!("  about: {}\n", ids.join(", ")));
            }
            if let Some(a) = &r.annotation {
                text.push_str(&format!("  annotation: {a}\n"));
            }
        }
        ToolResult::text(text)
    }

    fn requests_resolve(&self, args: &Value) -> ToolResult {
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("id is required"),
        };
        let status = match args
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("done")
        {
            "dismissed" => RequestStatus::Dismissed,
            _ => RequestStatus::Done,
        };

        let requests = match storage::load_requests(&self.project_dir) {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        match requests.into_iter().find(|r| r.id == id) {
            Some(mut r) => {
                r.status = status;
                match storage::save_request(&self.project_dir, &r) {
                    Ok(_) => ToolResult::text(format!("Marked {id} as {status:?}.")),
                    Err(e) => ToolResult::error(e.to_string()),
                }
            }
            None => ToolResult::error(format!("no request '{id}'")),
        }
    }

    fn save(&self) -> Result<(), String> {
        if !self.autosave {
            return Ok(());
        }
        storage::save_project(&self.project_dir, self.session.document()).map_err(|e| e.to_string())
    }
}
