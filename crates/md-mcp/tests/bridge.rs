//! The bridge's regression test.
//!
//! Drives a full MCP conversation the way a model would: handshake, list the tools,
//! read the document, change it, animate it, look at the result, undo. If this breaks,
//! the AI half of the product is broken, whatever else still passes.

use md_anim::{Origin, Registry};
use md_doc::node::{NodeKind, RectGeometry};
use md_doc::{Document, Node, NodeId, Paint};
use md_mcp::protocol::Request;
use md_mcp::Server;
use serde_json::{json, Value};
use std::path::PathBuf;

fn std_animations() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/md-anim-std/animations")
        .canonicalize()
        .expect("the standard animation library should be in the repository")
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("md-mcp-bridge")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Three cards and a heading — enough to exercise selectors, stagger and layout.
fn document() -> Document {
    let mut d = Document::new("Bridge test");
    d.pages[0].root.id = NodeId::from_static("nd_root");
    d.pages[0].background = Some(Paint::solid("#0b1020").unwrap());

    for i in 0..3 {
        let id = NodeId::parse(format!("nd_card{i}")).unwrap();
        let mut card = Node::new(
            id,
            NodeKind::Rect(RectGeometry {
                width: 300.0,
                height: 180.0,
                corner_radius: [16.0; 4],
            }),
        )
        .with_role("card")
        .with_fill(Paint::solid("#1e293b").unwrap());
        card.transform = md_doc::Transform::translate(100.0 + 340.0 * i as f64, 320.0);
        d.pages[0].root.children.push(card);
    }
    d
}

fn server(name: &str) -> Server {
    let dir = tmpdir(name);
    let doc = document();
    md_doc::storage::save_project(&dir, &doc).unwrap();

    let mut registry = Registry::new();
    registry.load_dir(&std_animations(), Origin::Standard);
    assert!(!registry.is_empty());

    let mut s = Server::with_document(&dir, doc, registry);
    s.autosave = true;
    s
}

/// Send a JSON-RPC request and return the `result`, failing loudly on an error reply.
fn rpc(server: &mut Server, method: &str, params: Value) -> Value {
    let request: Request = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    }))
    .unwrap();

    let response = server.handle(request).expect("expected a response");
    let value = serde_json::to_value(&response).unwrap();
    assert!(value.get("error").is_none(), "{method} failed: {value}");
    value["result"].clone()
}

/// Call a tool and return its text content, asserting it did not report an error.
fn call(server: &mut Server, tool: &str, args: Value) -> String {
    let result = rpc(server, "tools/call", json!({ "name": tool, "arguments": args }));
    let text = text_of(&result);
    assert_eq!(
        result["isError"], json!(false),
        "{tool} reported an error: {text}"
    );
    text
}

/// Call a tool expecting it to refuse, and return the reason.
fn call_expecting_error(server: &mut Server, tool: &str, args: Value) -> String {
    let result = rpc(server, "tools/call", json!({ "name": tool, "arguments": args }));
    assert_eq!(result["isError"], json!(true), "{tool} unexpectedly succeeded");
    text_of(&result)
}

fn text_of(result: &Value) -> String {
    result["content"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

#[test]
fn the_handshake_advertises_the_server_and_briefs_the_model() {
    let mut s = server("handshake");
    let result = rpc(&mut s, "initialize", json!({ "protocolVersion": "2024-11-05" }));

    assert_eq!(result["serverInfo"]["name"], "master-design");
    assert!(result["capabilities"]["tools"].is_object());

    // The instructions are the model's whole orientation; they must name the document
    // and the roles it can address.
    let instructions = result["instructions"].as_str().unwrap();
    assert!(instructions.contains("Bridge test"), "got {instructions}");
    assert!(instructions.contains("@card"), "roles should be listed: {instructions}");
    assert!(instructions.contains("doc_snapshot"), "got {instructions}");
}

#[test]
fn a_newer_protocol_revision_is_accepted_rather_than_refused() {
    let mut s = server("version");
    let result = rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    assert_eq!(result["protocolVersion"], "2025-06-18");
}

#[test]
fn notifications_get_no_reply() {
    let mut s = server("notify");
    let request: Request = serde_json::from_value(json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }))
    .unwrap();
    assert!(s.handle(request).is_none());
}

#[test]
fn every_advertised_tool_has_a_schema_and_a_description() {
    let mut s = server("tools");
    let result = rpc(&mut s, "tools/list", json!({}));
    let tools = result["tools"].as_array().unwrap();
    assert!(tools.len() >= 10, "only {} tools", tools.len());

    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert!(
            tool["inputSchema"]["type"] == "object",
            "{name} has no object input schema"
        );
        let description = tool["description"].as_str().unwrap_or("");
        assert!(
            description.len() > 40,
            "{name}'s description is too thin for a model to choose it: {description:?}"
        );
    }
}

#[test]
fn an_unknown_method_is_a_protocol_error() {
    let mut s = server("unknown");
    let request: Request = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 7, "method": "does/notexist"
    }))
    .unwrap();
    let response = s.handle(request).unwrap();
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["error"]["code"], -32601);
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[test]
fn describe_shows_the_tree_and_the_role_vocabulary() {
    let mut s = server("describe");
    let text = call(&mut s, "doc_describe", json!({}));
    assert!(text.contains("roles: @card×3"), "got {text}");
    assert!(text.contains("rect nd_card0"), "got {text}");
}

#[test]
fn query_returns_matching_nodes_with_their_properties() {
    let mut s = server("query");
    let text = call(&mut s, "doc_query", json!({ "selector": "@card" }));
    assert!(text.contains("\"matches\": 3"), "got {text}");
    assert!(text.contains("nd_card0"), "got {text}");
}

#[test]
fn a_selector_that_matches_nothing_says_what_to_do_next() {
    let mut s = server("nomatch");
    let text = call(&mut s, "doc_query", json!({ "selector": "@nonexistent" }));
    assert!(text.contains("matched nothing"), "got {text}");
    assert!(text.contains("doc_describe"), "should point somewhere useful: {text}");
}

#[test]
fn a_malformed_selector_is_refused_with_a_reason() {
    let mut s = server("badsel");
    let text = call_expecting_error(&mut s, "doc_query", json!({ "selector": "@@@" }));
    assert!(text.contains("selector"), "got {text}");
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

#[test]
fn a_patch_applies_and_is_written_to_disk() {
    let mut s = server("patch");
    let text = call(
        &mut s,
        "doc_patch",
        json!({
            "label": "Recolour the cards",
            "ops": [
                { "op": "node.update", "id": "nd_card0", "path": "fills.0.color", "value": "#ff0055" }
            ]
        }),
    );

    assert!(text.contains("Applied 1 operation"), "got {text}");
    assert!(text.contains("doc_snapshot"), "the model should be told to look: {text}");

    // Autosave is what makes the change appear on the person's canvas.
    let on_disk = md_doc::storage::load_project(
        &std::env::temp_dir().join("md-mcp-bridge").join(format!("patch-{}", std::process::id())),
    )
    .unwrap();
    let node = on_disk.node(&NodeId::from_static("nd_card0")).unwrap();
    match &node.fills[0] {
        Paint::Solid { color, .. } => assert_eq!(color.as_str(), "#ff0055"),
        other => panic!("expected a solid fill, got {other:?}"),
    }
}

#[test]
fn a_refused_patch_says_plainly_that_nothing_changed() {
    let mut s = server("refused");
    let text = call_expecting_error(
        &mut s,
        "doc_patch",
        json!({
            "ops": [
                { "op": "node.update", "id": "nd_card0", "path": "fills.0.color", "value": "#ff0055" },
                { "op": "node.delete", "id": "nd_ghost" }
            ]
        }),
    );

    // The natural assumption after an error is that some of it landed. It did not, and
    // a model that believes otherwise will make things worse trying to clean up.
    assert!(text.contains("Nothing was applied"), "got {text}");

    let node = s.document().node(&NodeId::from_static("nd_card0")).unwrap();
    match &node.fills[0] {
        Paint::Solid { color, .. } => assert_eq!(color.as_str(), "#1e293b", "first op was left applied"),
        other => panic!("expected a solid fill, got {other:?}"),
    }
}

#[test]
fn malformed_operations_come_back_with_an_example() {
    let mut s = server("badops");
    let text = call_expecting_error(&mut s, "doc_patch", json!({ "ops": [{ "nope": 1 }] }));
    assert!(text.contains("\"op\""), "the error should show the shape expected: {text}");
}

#[test]
fn an_ai_edit_is_undoable() {
    // The property the whole design rests on. If a model's change could not be taken
    // back with one keystroke, nobody would let it touch anything.
    let mut s = server("undo");
    let before = md_doc::to_canonical_string(s.document()).unwrap();

    call(
        &mut s,
        "doc_patch",
        json!({
            "label": "Widen a card",
            "ops": [{ "op": "node.update", "id": "nd_card1", "path": "width", "value": 500 }]
        }),
    );
    assert_ne!(md_doc::to_canonical_string(s.document()).unwrap(), before);

    let text = call(&mut s, "doc_undo", json!({}));
    assert!(text.contains("Widen a card"), "the undo should name the step: {text}");
    assert_eq!(
        md_doc::to_canonical_string(s.document()).unwrap(),
        before,
        "undo did not restore the document byte for byte"
    );

    call(&mut s, "doc_redo", json!({}));
    assert_ne!(md_doc::to_canonical_string(s.document()).unwrap(), before);
}

// ---------------------------------------------------------------------------
// Animating
// ---------------------------------------------------------------------------

#[test]
fn animations_list_with_their_parameters() {
    let mut s = server("animlist");
    let text = call(&mut s, "anim_list", json!({ "query": "stagger" }));
    assert!(text.contains("std/stagger-fade-up"), "got {text}");
    assert!(text.contains("distance"), "parameters must be listed: {text}");
    assert!(text.contains("[0..400]"), "ranges must be listed: {text}");
}

#[test]
fn applying_an_animation_adds_a_timeline_and_reports_what_it_did() {
    let mut s = server("animapply");
    let text = call(
        &mut s,
        "anim_apply",
        json!({
            "package": "std/stagger-fade-up",
            "selector": "@card",
            "params": { "distance": 48, "stagger": 0.12 }
        }),
    );

    assert!(text.contains("6 track(s)"), "3 cards × 2 properties: {text}");
    assert!(text.contains("triggered on view"), "got {text}");

    let page = &s.document().pages[0];
    assert_eq!(page.timelines.len(), 1);
    let source = page.timelines[0].source.as_ref().unwrap();
    assert_eq!(source.target, "@card", "the selector must be recorded for re-baking");
    assert_eq!(source.version, "1.0.0", "the version must be pinned");
}

#[test]
fn an_animation_applied_to_the_wrong_kind_of_node_is_refused_clearly() {
    let mut s = server("animkind");
    let text = call_expecting_error(
        &mut s,
        "anim_apply",
        json!({ "package": "std/draw-path", "selector": "@card" }),
    );
    assert!(text.contains("cannot apply to a rect"), "got {text}");
}

#[test]
fn an_out_of_range_parameter_is_refused() {
    let mut s = server("animparam");
    let text = call_expecting_error(
        &mut s,
        "anim_apply",
        json!({
            "package": "std/stagger-fade-up",
            "selector": "@card",
            "params": { "distance": 99999 }
        }),
    );
    assert!(text.contains("distance"), "got {text}");
}

// ---------------------------------------------------------------------------
// Seeing
// ---------------------------------------------------------------------------

fn png_of(result: &Value) -> Vec<u8> {
    use base64::Engine;
    let data = result["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "image")
        .expect("no image in the result")["data"]
        .as_str()
        .unwrap();
    base64::engine::general_purpose::STANDARD.decode(data).unwrap()
}

#[test]
fn a_snapshot_comes_back_as_a_real_png() {
    let mut s = server("snapshot");
    let result = rpc(&mut s, "tools/call", json!({ "name": "doc_snapshot", "arguments": {} }));
    assert_eq!(result["isError"], json!(false));

    let png = png_of(&result);
    assert_eq!(&png[1..4], b"PNG");
    assert!(png.len() > 1000, "suspiciously small image");
}

#[test]
fn the_model_can_watch_its_own_animation_run() {
    // The loop this server exists for: change something, then look at it moving.
    let mut s = server("watch");
    call(
        &mut s,
        "anim_apply",
        json!({ "package": "std/stagger-fade-up", "selector": "@card" }),
    );

    let start = rpc(
        &mut s,
        "tools/call",
        json!({ "name": "doc_snapshot", "arguments": { "time": 0.0, "width": 320 } }),
    );
    let middle = rpc(
        &mut s,
        "tools/call",
        json!({ "name": "doc_snapshot", "arguments": { "time": 0.3, "width": 320 } }),
    );
    let end = rpc(
        &mut s,
        "tools/call",
        json!({ "name": "doc_snapshot", "arguments": { "time": 5.0, "width": 320 } }),
    );

    let (a, b, c) = (png_of(&start), png_of(&middle), png_of(&end));
    assert_ne!(a, b, "nothing moved between the start and the middle");
    assert_ne!(b, c, "nothing moved between the middle and the end");
}

#[test]
fn a_snapshot_can_be_cropped_to_one_node() {
    let mut s = server("crop");
    let full = rpc(
        &mut s,
        "tools/call",
        json!({ "name": "doc_snapshot", "arguments": { "width": 400 } }),
    );
    let cropped = rpc(
        &mut s,
        "tools/call",
        json!({ "name": "doc_snapshot", "arguments": { "node": "nd_card0", "width": 400 } }),
    );
    assert_ne!(png_of(&full), png_of(&cropped));
}

#[test]
fn snapshotting_a_node_that_does_not_exist_is_refused() {
    let mut s = server("cropmissing");
    let text =
        call_expecting_error(&mut s, "doc_snapshot", json!({ "node": "nd_nowhere" }));
    assert!(text.contains("nd_nowhere"), "got {text}");
}

// ---------------------------------------------------------------------------
// Exporting, and the asynchronous half of the bridge
// ---------------------------------------------------------------------------

#[test]
fn export_writes_a_working_page() {
    let dir = tmpdir("export");
    let doc = document();
    md_doc::storage::save_project(&dir, &doc).unwrap();

    let mut registry = Registry::new();
    registry.load_dir(&std_animations(), Origin::Standard);
    let mut s = Server::with_document(&dir, doc, registry);
    s.autosave = true;

    call(&mut s, "anim_apply", json!({ "package": "std/pop-in", "selector": "@card" }));
    let text = call(&mut s, "export_build", json!({}));
    assert!(text.contains("Exported"), "got {text}");

    let html = std::fs::read_to_string(dir.join("dist/index.html")).unwrap();
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains("id=\"md-animations\""), "the runtime payload is missing");
    assert!(html.contains("data-md-fx=\"nd_card0\""), "the animation target is missing");
}

#[test]
fn selection_is_empty_and_says_where_else_to_look_when_no_studio_is_running() {
    let mut s = server("noselection");
    let text = call(&mut s, "selection_get", json!({}));
    assert!(text.contains("Nothing is selected"), "got {text}");
    assert!(text.contains("requests_list"), "should point at the async path: {text}");
}

#[test]
fn a_live_selection_tells_the_model_what_this_means() {
    let dir = tmpdir("selection");
    let doc = document();
    md_doc::storage::save_project(&dir, &doc).unwrap();

    md_doc::selection::save_selection(
        &dir,
        &md_doc::Selection {
            page: "index".into(),
            nodes: vec![NodeId::from_static("nd_card1")],
            note: "make this bounce".into(),
            annotation: Some(".md/scribble.png".into()),
            viewport: Some([0.0, 200.0, 800.0, 400.0]),
            updated_at: 1,
        },
    )
    .unwrap();

    let mut s = Server::with_document(&dir, doc, Registry::new());
    let text = call(&mut s, "selection_get", json!({}));

    assert!(text.contains("make this bounce"), "got {text}");
    assert!(text.contains("nd_card1"), "got {text}");
    assert!(text.contains("scribble.png"), "the annotation should be surfaced: {text}");
    assert!(text.contains("region"), "the viewport should be actionable: {text}");
}

#[test]
fn queued_requests_survive_the_studio_being_closed() {
    // The mobile story: a note written on a phone, with no agent attached, waits in the
    // project and is picked up by a desktop session later.
    let dir = tmpdir("requests");
    let doc = document();
    md_doc::storage::save_project(&dir, &doc).unwrap();

    md_doc::storage::save_request(
        &dir,
        &md_doc::storage::Request {
            id: "req_1".into(),
            created_at: 1,
            note: "the cards feel too static".into(),
            page: "index".into(),
            selection: vec![NodeId::from_static("nd_card0")],
            annotation: None,
            status: md_doc::storage::RequestStatus::Open,
        },
    )
    .unwrap();

    let mut s = Server::with_document(&dir, doc, Registry::new());

    let text = call(&mut s, "requests_list", json!({}));
    assert!(text.contains("too static"), "got {text}");
    assert!(text.contains("nd_card0"), "got {text}");

    call(&mut s, "requests_resolve", json!({ "id": "req_1", "status": "done" }));
    let after = call(&mut s, "requests_list", json!({}));
    assert!(after.contains("No queued requests"), "got {after}");
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

#[test]
fn the_stdio_transport_handles_a_whole_conversation() {
    let mut s = server("stdio");

    let script = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "doc_describe", "arguments": {} }
        }),
    ]
    .iter()
    .map(|v| v.to_string())
    .collect::<Vec<_>>()
    .join("\n");

    let mut output = Vec::new();
    s.serve(std::io::Cursor::new(script), &mut output).unwrap();

    let lines: Vec<&str> = std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();

    // Three requests, one notification, so three replies.
    assert_eq!(lines.len(), 3, "got {lines:#?}");
    for (line, id) in lines.iter().zip([1, 2, 3]) {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], id);
        assert!(value.get("error").is_none(), "{line}");
    }
}

#[test]
fn a_malformed_line_gets_a_parse_error_rather_than_a_crash() {
    let mut s = server("garbage");
    let mut output = Vec::new();
    s.serve(std::io::Cursor::new("{ not json\n"), &mut output).unwrap();

    let value: Value = serde_json::from_str(std::str::from_utf8(&output).unwrap()).unwrap();
    assert_eq!(value["error"]["code"], -32700);
}
