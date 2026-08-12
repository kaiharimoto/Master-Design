//! `md` — the Master Design command line.
//!
//! Everything the studio can do to a document, without the studio: create a project,
//! read it, patch it, animate it, export it, render a PNG of it, or serve it over MCP.
//!
//! That is not a convenience. It is what makes the AI half of this system testable in
//! CI and usable on a machine with no display — and it is the reason the animation
//! format is declarative and the exporter is written in Rust.

mod paths;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use md_doc::digest::{digest, DigestOptions};
use md_doc::history::Session;
use md_doc::patch::Op;
use md_doc::storage::{self, Request, RequestStatus};
use md_doc::{Document, NodeId, Selector};
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "md",
    version,
    about = "Design and build graphic animated websites.",
    long_about = None
)]
struct Cli {
    /// Directory holding the standard animation library.
    #[arg(long, global = true, value_name = "DIR")]
    animations: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new project.
    New {
        /// Directory to create.
        path: PathBuf,
        /// Project name. Defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
        /// Page size in document units.
        #[arg(long, default_value_t = 1440.0)]
        width: f64,
        #[arg(long, default_value_t = 900.0)]
        height: f64,
    },

    /// Print a compact outline of the document.
    Describe {
        project: PathBuf,
        #[arg(long)]
        page: Option<String>,
        #[arg(long, default_value_t = 12)]
        max_depth: usize,
    },

    /// Find nodes with a selector.
    Query {
        project: PathBuf,
        /// e.g. '@card', 'type:text', '#nd_abc'
        selector: String,
        #[arg(long)]
        page: Option<String>,
        /// Print each matching node's full JSON.
        #[arg(long)]
        full: bool,
    },

    /// Apply operations from a JSON file, or from stdin with `-`.
    Patch {
        project: PathBuf,
        ops: PathBuf,
        #[arg(long, default_value = "CLI patch")]
        label: String,
    },

    /// Build the static site.
    Export {
        project: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        page: Option<String>,
    },

    /// Render a PNG of a page.
    Snapshot {
        project: PathBuf,
        #[arg(long, default_value = "snapshot.png")]
        out: PathBuf,
        #[arg(long)]
        page: Option<String>,
        /// Crop to a node.
        #[arg(long)]
        node: Option<String>,
        /// Seconds into the page's animations.
        #[arg(long)]
        time: Option<f64>,
        #[arg(long, default_value_t = 1024)]
        width: u32,
        /// Solve the layout at this document width first — 390 for a phone, 768 for a
        /// tablet. Without it the design renders as authored.
        #[arg(long)]
        at_width: Option<f64>,
    },

    /// Work with animation packages.
    #[command(subcommand)]
    Anim(AnimCommand),

    /// Queue a note for an AI session to pick up later.
    Request {
        project: PathBuf,
        note: String,
        #[arg(long)]
        page: Option<String>,
        /// Node ids the note is about.
        #[arg(long, value_delimiter = ',')]
        nodes: Vec<String>,
    },

    /// Serve the project over the Model Context Protocol on stdin/stdout.
    Mcp {
        project: PathBuf,
        /// Do not write changes back to disk.
        #[arg(long)]
        read_only: bool,
    },
}

#[derive(Subcommand)]
enum AnimCommand {
    /// List installed animation packages.
    List {
        #[arg(long)]
        query: Option<String>,
    },
    /// Apply an animation to the nodes a selector matches.
    Apply {
        project: PathBuf,
        /// Package id, e.g. std/stagger-fade-up
        package: String,
        /// e.g. '@card'
        selector: String,
        /// Parameter overrides as JSON, e.g. '{"distance":48}'
        #[arg(long)]
        params: Option<String>,
        #[arg(long)]
        page: Option<String>,
    },
}

fn main() {
    if let Err(e) = run() {
        // `{e:#}` prints the whole context chain, which is usually where the useful part
        // of the message lives.
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let std_animations = paths::standard_animations(cli.animations.as_deref());

    match cli.command {
        Command::New {
            path,
            name,
            width,
            height,
        } => cmd_new(path, name, width, height),
        Command::Describe {
            project,
            page,
            max_depth,
        } => {
            let doc = load(&project)?;
            print!(
                "{}",
                digest(
                    &doc,
                    &DigestOptions {
                        page,
                        max_depth,
                        ..Default::default()
                    }
                )
            );
            Ok(())
        }
        Command::Query {
            project,
            selector,
            page,
            full,
        } => cmd_query(&project, &selector, page.as_deref(), full),
        Command::Patch {
            project,
            ops,
            label,
        } => cmd_patch(&project, &ops, &label),
        Command::Export { project, out, page } => cmd_export(&project, out, page),
        Command::Snapshot {
            project,
            out,
            page,
            node,
            time,
            width,
            at_width,
        } => cmd_snapshot(&project, out, page, node, time, width, at_width),
        Command::Anim(AnimCommand::List { query }) => cmd_anim_list(std_animations, query),
        Command::Anim(AnimCommand::Apply {
            project,
            package,
            selector,
            params,
            page,
        }) => cmd_anim_apply(&project, std_animations, &package, &selector, params, page),
        Command::Request {
            project,
            note,
            page,
            nodes,
        } => cmd_request(&project, note, page, nodes),
        Command::Mcp { project, read_only } => cmd_mcp(&project, std_animations, read_only),
    }
}

fn load(path: &std::path::Path) -> Result<Document> {
    let root = paths::project_root(path)?;
    storage::load_project(&root).with_context(|| format!("reading {}", root.display()))
}

fn cmd_new(path: PathBuf, name: Option<String>, width: f64, height: f64) -> Result<()> {
    if path.exists() && storage::is_project_dir(&path) {
        return Err(anyhow!("{} is already a project", path.display()));
    }

    let name = name.unwrap_or_else(|| {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .replace(['-', '_'], " ")
    });

    let doc = Document::sized(&name, width, height)?;

    // `md` by name, not by absolute path: this file is committed and the project is meant
    // to travel between machines. Anyone running `md new` has `md` on their PATH by
    // definition.
    let extras = storage::scaffold_project(&path, &doc, "md")?;
    let attached = extras
        .iter()
        .any(|p| p.file_name().and_then(|n| n.to_str()) == Some(storage::MCP_CONFIG_FILE));

    println!("Created \"{name}\" at {}", path.display());
    println!("  {width}×{height}");
    println!("\nNext:");
    println!("  md describe {}", path.display());
    if attached {
        println!("  claude                        # in that folder — the MCP server is already configured");
    }
    println!("  md mcp {}    # or attach a model by hand", path.display());
    Ok(())
}

fn cmd_query(
    project: &std::path::Path,
    selector: &str,
    page: Option<&str>,
    full: bool,
) -> Result<()> {
    let doc = load(project)?;
    let parsed = Selector::parse(selector)?;

    let ids = match page {
        Some(p) => parsed.select_in_page(&doc, p)?,
        None => parsed.select(&doc),
    };

    if ids.is_empty() {
        println!("'{selector}' matched nothing.");
        return Ok(());
    }

    for id in &ids {
        let node = match doc.node(id) {
            Some(n) => n,
            None => continue,
        };
        if full {
            println!("{}", md_doc::to_canonical_string(node)?);
        } else {
            println!(
                "{:<8} {} {}{}",
                node.kind.type_name(),
                node.id,
                node.display_name(),
                if node.roles.is_empty() {
                    String::new()
                } else {
                    format!("  @{}", node.roles.join(" @"))
                }
            );
        }
    }
    eprintln!("\n{} match(es)", ids.len());
    Ok(())
}

fn cmd_patch(project: &std::path::Path, ops_path: &std::path::Path, label: &str) -> Result<()> {
    let root = paths::project_root(project)?;
    let doc = storage::load_project(&root)?;

    let raw = if ops_path == std::path::Path::new("-") {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        std::fs::read_to_string(ops_path)
            .with_context(|| format!("reading {}", ops_path.display()))?
    };

    // Accept either a bare array of operations or `{ "ops": [...] }`, since both are
    // natural things to have lying around.
    let value: serde_json::Value = serde_json::from_str(&raw).context("parsing operations")?;
    let array = match value {
        serde_json::Value::Array(a) => serde_json::Value::Array(a),
        serde_json::Value::Object(ref o) if o.contains_key("ops") => o["ops"].clone(),
        _ => {
            return Err(anyhow!(
                "expected an array of operations, or an object with an 'ops' key"
            ))
        }
    };

    let ops: Vec<Op> = serde_json::from_value(array).context("reading operations")?;
    let mut session = Session::new(doc);
    let report = session.apply(label, ops)?;

    storage::save_project(&root, session.document())?;

    println!("Applied {} operation(s).", report.applied);
    for id in &report.touched {
        println!("  touched {id}");
    }
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
    Ok(())
}

fn cmd_export(project: &std::path::Path, out: Option<PathBuf>, page: Option<String>) -> Result<()> {
    let root = paths::project_root(project)?;
    let doc = storage::load_project(&root)?;
    let out = out.unwrap_or_else(|| root.join("dist"));

    let opts = md_emit::ExportOptions {
        assets_from: Some(root.join(storage::ASSETS_DIR)),
        only_page: page,
        ..Default::default()
    };

    let report = md_emit::export(&doc, &out, &opts)?;

    println!("Exported to {}", out.display());
    for f in &report.files {
        println!("  {}", f.display());
    }
    println!("{} bytes", report.bytes);
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
    Ok(())
}

fn cmd_snapshot(
    project: &std::path::Path,
    out: PathBuf,
    page: Option<String>,
    node: Option<String>,
    time: Option<f64>,
    width: u32,
    at_width: Option<f64>,
) -> Result<()> {
    let doc = load(project)?;
    let page_key = page.unwrap_or_else(|| {
        doc.pages
            .first()
            .map(|p| p.slug.clone())
            .unwrap_or_default()
    });

    let opts = md_mcp::render::SnapshotOptions {
        width,
        time,
        node: node.map(NodeId::parse).transpose()?,
        region: None,
        at_width,
    };

    let (png, w, h) = md_mcp::render::snapshot(&doc, &page_key, &opts).map_err(|e| anyhow!(e))?;
    std::fs::write(&out, &png).with_context(|| format!("writing {}", out.display()))?;

    println!("Wrote {} ({w}×{h})", out.display());
    Ok(())
}

fn cmd_anim_list(std_animations: Option<PathBuf>, query: Option<String>) -> Result<()> {
    let registry = registry(std_animations.as_deref(), None);
    let packages = registry.search(query.as_deref().unwrap_or(""));

    if packages.is_empty() {
        println!("No animation packages found.");
        println!("Point --animations at a directory of packages, or set MD_ANIMATIONS.");
        return Ok(());
    }

    for pkg in packages {
        let m = &pkg.manifest;
        println!("{:<26} v{:<8} {}", m.id, m.version, m.title);
        println!("  {}", m.description);
        println!(
            "  {} · {} · {}",
            m.category.as_str(),
            md_anim::bake::kind_summary(m),
            pkg.origin.as_str()
        );
        for p in &m.params {
            println!("    {:<12} default {}", p.key, p.default);
        }
        println!();
    }
    Ok(())
}

fn cmd_anim_apply(
    project: &std::path::Path,
    std_animations: Option<PathBuf>,
    package: &str,
    selector: &str,
    params: Option<String>,
    page: Option<String>,
) -> Result<()> {
    let root = paths::project_root(project)?;
    let doc = storage::load_project(&root)?;
    let registry = registry(std_animations.as_deref(), Some(&root));

    let params = match params {
        Some(raw) => serde_json::from_str::<serde_json::Value>(&raw)
            .context("parsing --params")?
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("--params must be a JSON object"))?,
        None => Default::default(),
    };

    let page_key = page.unwrap_or_else(|| {
        doc.pages
            .first()
            .map(|p| p.slug.clone())
            .unwrap_or_default()
    });

    let timeline =
        md_anim::apply_to_selector(&doc, &registry, package, selector, params, Some(&page_key))?;

    let tracks = timeline.tracks.len();
    let duration = timeline.duration;

    let mut session = Session::new(doc);
    session.apply(
        format!("Apply {package}"),
        vec![Op::TimelineSet {
            page: page_key.clone(),
            timeline: Box::new(timeline),
        }],
    )?;
    storage::save_project(&root, session.document())?;

    println!(
        "Applied {package} to '{selector}' on '{page_key}': {tracks} track(s) over {}s",
        md_geom::fmt_coord(duration)
    );
    Ok(())
}

fn cmd_request(
    project: &std::path::Path,
    note: String,
    page: Option<String>,
    nodes: Vec<String>,
) -> Result<()> {
    let root = paths::project_root(project)?;
    let doc = storage::load_project(&root)?;

    let ids: Result<Vec<NodeId>> = nodes
        .iter()
        .map(|n| NodeId::parse(n).map_err(Into::into))
        .collect();

    let created = storage::now_millis();
    let request = Request {
        id: format!("req_{created}"),
        created_at: created,
        note,
        page: page.unwrap_or_else(|| {
            doc.pages
                .first()
                .map(|p| p.slug.clone())
                .unwrap_or_default()
        }),
        selection: ids?,
        annotation: None,
        status: RequestStatus::Open,
    };

    let path = storage::save_request(&root, &request)?;
    println!("Queued {} at {}", request.id, path.display());
    Ok(())
}

fn cmd_mcp(
    project: &std::path::Path,
    std_animations: Option<PathBuf>,
    read_only: bool,
) -> Result<()> {
    let root = paths::project_root(project)?;
    let mut server =
        md_mcp::Server::open(&root, std_animations.as_deref()).map_err(|e| anyhow!("{e}"))?;
    server.autosave = !read_only;

    // Anything printed to stdout would be read as a protocol message, so status goes to
    // stderr — which is also where an MCP client shows server logs.
    eprintln!(
        "master-design MCP server on {}{}",
        root.display(),
        if read_only { " (read-only)" } else { "" }
    );

    server.serve_stdio()?;
    Ok(())
}

fn registry(
    std_animations: Option<&std::path::Path>,
    project: Option<&std::path::Path>,
) -> md_anim::Registry {
    let mut reg = md_anim::Registry::new();
    if let Some(dir) = std_animations {
        reg.load_dir(dir, md_anim::Origin::Standard);
    }
    if let Some(root) = project {
        reg.load_dir(
            &root.join(storage::ANIMATIONS_DIR),
            md_anim::Origin::Project,
        );
    }
    for problem in &reg.problems {
        eprintln!("warning: {problem}");
    }
    reg
}
