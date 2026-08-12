//! The patch protocol.
//!
//! Every change to a document — from a mouse drag, from a menu command, from an AI
//! model over MCP — is expressed as a list of [`Op`]s. Nothing mutates a document any
//! other way. That single rule is what buys:
//!
//! * **One history.** An AI edit is undone with the same keystroke as a mouse drag,
//!   because both produced inverse ops that went on the same stack.
//! * **One validator.** A model cannot write a node the GUI could not have written.
//! * **Atomicity.** Ops are applied to a copy; a failure part-way leaves the document
//!   untouched rather than half-edited.
//! * **A real audit trail.** The op list is a description of intent, not a diff of bytes.
//!
//! Applying an op returns the ops that undo it. Callers that want history should go
//! through [`crate::history::Session`] rather than calling [`apply_ops`] directly.

use crate::anim::Timeline;
use crate::canonical::ensure_finite;
use crate::document::{Document, NodeLocation, Page};
use crate::error::{DocError, Result};
use crate::id::{NodeId, PageId, TimelineId};
use crate::node::{Node, NodeKind, PathGeometry};
use crate::transform::Transform;
use md_geom::BoolOp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// `node.insert` carries a whole subtree and is much larger than the other variants.
// Boxing it would even the sizes out and cost every call site — including every one an
// AI writes as JSON — an extra indirection, for a type that is constructed a handful of
// times per patch and never in a hot loop.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    /// Add a node to a parent. `index` appends when omitted.
    #[serde(rename = "node.insert", rename_all = "camelCase")]
    NodeInsert {
        parent: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        node: Node,
    },

    #[serde(rename = "node.delete", rename_all = "camelCase")]
    NodeDelete { id: NodeId },

    /// Set one property, addressed by a dotted path such as `fills.0.color` or
    /// `width`. A `null` value clears an optional property.
    #[serde(rename = "node.update", rename_all = "camelCase")]
    NodeUpdate {
        id: NodeId,
        path: String,
        value: Value,
    },

    /// Reparent and/or reorder. Also how "bring to front" is expressed.
    #[serde(rename = "node.move", rename_all = "camelCase")]
    NodeMove {
        id: NodeId,
        parent: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },

    /// Combine two or more sibling nodes into a single path.
    #[serde(rename = "path.boolean", rename_all = "camelCase")]
    PathBoolean {
        ids: Vec<NodeId>,
        mode: BoolOp,
        /// Id for the resulting node. Generated when omitted — but supplying one makes
        /// the operation reproducible, which matters for tests and for a model that
        /// wants to refer to the result in a later op of the same patch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_id: Option<NodeId>,
    },

    #[serde(rename = "page.insert", rename_all = "camelCase")]
    PageInsert {
        page: Box<Page>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },

    #[serde(rename = "page.delete", rename_all = "camelCase")]
    PageDelete { page: String },

    /// Set a property on a page — `name`, `slug`, `width`, `background`, and so on.
    #[serde(rename = "page.update", rename_all = "camelCase")]
    PageUpdate {
        page: String,
        path: String,
        value: Value,
    },

    /// Add or replace a timeline, matched by id.
    #[serde(rename = "timeline.set", rename_all = "camelCase")]
    TimelineSet {
        page: String,
        timeline: Box<Timeline>,
    },

    #[serde(rename = "timeline.remove", rename_all = "camelCase")]
    TimelineRemove { page: String, id: TimelineId },

    /// Set a design token, e.g. `colors.accent`.
    #[serde(rename = "tokens.set", rename_all = "camelCase")]
    TokensSet { path: String, value: Value },
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::NodeInsert { .. } => "node.insert",
            Op::NodeDelete { .. } => "node.delete",
            Op::NodeUpdate { .. } => "node.update",
            Op::NodeMove { .. } => "node.move",
            Op::PathBoolean { .. } => "path.boolean",
            Op::PageInsert { .. } => "page.insert",
            Op::PageDelete { .. } => "page.delete",
            Op::PageUpdate { .. } => "page.update",
            Op::TimelineSet { .. } => "timeline.set",
            Op::TimelineRemove { .. } => "timeline.remove",
            Op::TokensSet { .. } => "tokens.set",
        }
    }
}

/// What a patch did, in a form worth handing back to whoever sent it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchReport {
    pub applied: usize,
    /// Nodes created, changed, moved or removed.
    pub touched: Vec<NodeId>,
    /// Things that succeeded but are probably not what was meant.
    pub warnings: Vec<String>,
}

impl PatchReport {
    fn touch(&mut self, id: &NodeId) {
        if !self.touched.contains(id) {
            self.touched.push(id.clone());
        }
    }
}

/// Apply a list of ops atomically, returning the ops that undo them.
///
/// The returned list is already ordered for replay: applying it in order restores the
/// document. On any failure the document is left exactly as it was.
pub fn apply_ops(doc: &mut Document, ops: &[Op]) -> Result<(Vec<Op>, PatchReport)> {
    // Work on a copy so a failure half-way cannot leave a partly-edited document.
    // Documents are small enough that this is cheaper than any rollback machinery, and
    // it is a great deal harder to get wrong.
    let mut working = doc.clone();
    let mut report = PatchReport::default();
    let mut per_op: Vec<Vec<Op>> = Vec::with_capacity(ops.len());

    for op in ops {
        let inverse = apply_one(&mut working, op, &mut report)?;
        per_op.push(inverse);
        report.applied += 1;
    }

    // Undo runs backwards through the ops.
    let mut inverse = Vec::new();
    for group in per_op.into_iter().rev() {
        inverse.extend(group);
    }

    *doc = working;
    Ok((inverse, report))
}

fn apply_one(doc: &mut Document, op: &Op, report: &mut PatchReport) -> Result<Vec<Op>> {
    match op {
        Op::NodeInsert {
            parent,
            index,
            node,
        } => insert_node(doc, parent, *index, node, report),
        Op::NodeDelete { id } => delete_node(doc, id, report),
        Op::NodeUpdate { id, path, value } => update_node(doc, id, path, value, report),
        Op::NodeMove { id, parent, index } => move_node(doc, id, parent, *index, report),
        Op::PathBoolean {
            ids,
            mode,
            result_id,
        } => path_boolean(doc, ids, *mode, result_id.clone(), report),
        Op::PageInsert { page, index } => insert_page(doc, page, *index),
        Op::PageDelete { page } => delete_page(doc, page),
        Op::PageUpdate { page, path, value } => update_page(doc, page, path, value),
        Op::TimelineSet { page, timeline } => set_timeline(doc, page, timeline),
        Op::TimelineRemove { page, id } => remove_timeline(doc, page, id),
        Op::TokensSet { path, value } => set_tokens(doc, path, value),
    }
}

// ---------------------------------------------------------------------------
// Node operations
// ---------------------------------------------------------------------------

fn insert_node(
    doc: &mut Document,
    parent: &NodeId,
    index: Option<usize>,
    node: &Node,
    report: &mut PatchReport,
) -> Result<Vec<Op>> {
    // A non-finite coordinate has no JSON spelling and would be written as `null`, so it
    // is refused here rather than at save time, by which point the edit that produced it
    // is long out of sight.
    ensure_finite(node)?;

    // Nothing in the incoming subtree may collide with an id already in the document,
    // or later ops addressing that id would be ambiguous.
    for id in node.all_ids() {
        if doc.node(&id).is_some() {
            return Err(DocError::DuplicateNode(id));
        }
    }

    let loc = doc
        .locate(parent)
        .ok_or_else(|| DocError::NodeNotFound(parent.clone()))?;
    let parent_node = doc
        .node_at_mut(&loc)
        .ok_or_else(|| DocError::NodeNotFound(parent.clone()))?;
    if !parent_node.is_container() {
        return Err(DocError::NotAContainer(parent.clone()));
    }

    let len = parent_node.children.len();
    let at = index.unwrap_or(len);
    if at > len {
        return Err(DocError::IndexOutOfRange { index: at, len });
    }

    parent_node.children.insert(at, node.clone());
    report.touch(&node.id);
    Ok(vec![Op::NodeDelete {
        id: node.id.clone(),
    }])
}

fn delete_node(doc: &mut Document, id: &NodeId, report: &mut PatchReport) -> Result<Vec<Op>> {
    let loc = doc
        .locate(id)
        .ok_or_else(|| DocError::NodeNotFound(id.clone()))?;
    if loc.is_root() {
        return Err(DocError::RootIsImmovable);
    }
    let (parent_loc, index) = parent_and_index(&loc)?;
    let parent_id = doc
        .node_at(&parent_loc)
        .ok_or_else(|| DocError::NodeNotFound(id.clone()))?
        .id
        .clone();

    let parent = doc
        .node_at_mut(&parent_loc)
        .ok_or_else(|| DocError::NodeNotFound(id.clone()))?;
    let removed = parent.children.remove(index);
    report.touch(id);

    Ok(vec![Op::NodeInsert {
        parent: parent_id,
        index: Some(index),
        node: removed,
    }])
}

fn update_node(
    doc: &mut Document,
    id: &NodeId,
    path: &str,
    value: &Value,
    report: &mut PatchReport,
) -> Result<Vec<Op>> {
    let node = doc
        .node_mut(id)
        .ok_or_else(|| DocError::NodeNotFound(id.clone()))?;

    let mut tree = serde_json::to_value(&*node)?;
    let previous = set_path(&mut tree, path, value.clone())?;

    // Round-tripping through the typed model is the validation step: a bad colour, an
    // unknown enum or a wrong-shaped value fails here rather than reaching a file.
    let updated: Node = serde_json::from_value(tree).map_err(|e| DocError::InvalidValue {
        path: path.to_string(),
        reason: e.to_string(),
    })?;

    if updated.id != *id {
        return Err(DocError::InvalidValue {
            path: path.to_string(),
            reason: "a node's id cannot be changed; delete and re-insert instead".into(),
        });
    }

    *node = updated;
    report.touch(id);

    Ok(vec![Op::NodeUpdate {
        id: id.clone(),
        path: path.to_string(),
        // A property that did not exist is restored by clearing it again.
        value: previous.unwrap_or(Value::Null),
    }])
}

fn move_node(
    doc: &mut Document,
    id: &NodeId,
    new_parent: &NodeId,
    index: Option<usize>,
    report: &mut PatchReport,
) -> Result<Vec<Op>> {
    let loc = doc
        .locate(id)
        .ok_or_else(|| DocError::NodeNotFound(id.clone()))?;
    if loc.is_root() {
        return Err(DocError::RootIsImmovable);
    }

    // Moving a node inside its own subtree would detach that subtree from the document.
    let subtree = doc.require_node(id)?;
    if subtree.find(new_parent).is_some() {
        return Err(DocError::CyclicMove {
            parent: id.clone(),
            child: new_parent.clone(),
        });
    }

    let target_loc = doc
        .locate(new_parent)
        .ok_or_else(|| DocError::NodeNotFound(new_parent.clone()))?;
    if !doc
        .node_at(&target_loc)
        .map(|n| n.is_container())
        .unwrap_or(false)
    {
        return Err(DocError::NotAContainer(new_parent.clone()));
    }

    let (old_parent_loc, old_index) = parent_and_index(&loc)?;
    let old_parent_id = doc
        .node_at(&old_parent_loc)
        .ok_or_else(|| DocError::NodeNotFound(id.clone()))?
        .id
        .clone();

    let node = {
        let parent = doc
            .node_at_mut(&old_parent_loc)
            .ok_or_else(|| DocError::NodeNotFound(id.clone()))?;
        parent.children.remove(old_index)
    };

    // The removal may have shifted indices inside the destination, so re-locate it.
    let target_loc = doc
        .locate(new_parent)
        .ok_or_else(|| DocError::NodeNotFound(new_parent.clone()))?;
    let target = doc
        .node_at_mut(&target_loc)
        .ok_or_else(|| DocError::NodeNotFound(new_parent.clone()))?;
    let len = target.children.len();
    let at = index.unwrap_or(len).min(len);
    target.children.insert(at, node);

    report.touch(id);
    Ok(vec![Op::NodeMove {
        id: id.clone(),
        parent: old_parent_id,
        index: Some(old_index),
    }])
}

fn path_boolean(
    doc: &mut Document,
    ids: &[NodeId],
    mode: BoolOp,
    result_id: Option<NodeId>,
    report: &mut PatchReport,
) -> Result<Vec<Op>> {
    if ids.len() < 2 {
        return Err(DocError::InvalidValue {
            path: "ids".into(),
            reason: "a boolean operation needs at least two nodes".into(),
        });
    }

    // Every operand must share a parent: without a common coordinate space there is no
    // single answer to where the result belongs.
    let locs: Vec<NodeLocation> = ids
        .iter()
        .map(|id| {
            doc.locate(id)
                .ok_or_else(|| DocError::NodeNotFound(id.clone()))
        })
        .collect::<Result<_>>()?;

    let first_parent = locs[0].parent().ok_or(DocError::RootIsImmovable)?;
    if locs
        .iter()
        .any(|l| l.parent().as_ref() != Some(&first_parent))
    {
        return Err(DocError::InvalidValue {
            path: "ids".into(),
            reason: "all nodes in a boolean operation must share a parent".into(),
        });
    }

    // Flatten each operand into parent space so their transforms are accounted for.
    let mut geometry = Vec::with_capacity(ids.len());
    for id in ids {
        let node = doc.require_node(id)?;
        let d = node.geometry_path().ok_or_else(|| DocError::InvalidValue {
            path: "ids".into(),
            reason: format!("{} has no outline to combine", node.display_name()),
        })?;
        geometry.push(md_geom::transform_path(&d, node.transform.0)?);
    }

    let combined = md_geom::boolean_op(&geometry, mode, md_geom::DEFAULT_TOLERANCE, true)?;

    // The first operand's appearance and position in the stack carry over — that is
    // what every vector editor does, and what a designer expects.
    let template = doc.require_node(&ids[0])?.clone();
    let insert_index = locs[0].index_in_parent().unwrap_or(0);
    let parent_id = doc
        .node_at(&first_parent)
        .ok_or_else(|| DocError::NodeNotFound(ids[0].clone()))?
        .id
        .clone();

    let mut result = Node::new(
        result_id.unwrap_or_else(NodeId::new),
        NodeKind::Path(PathGeometry {
            d: combined,
            fill_rule: Default::default(),
        }),
    );
    result.name = template.name.clone();
    result.fills = template.fills.clone();
    result.strokes = template.strokes.clone();
    result.effects = template.effects.clone();
    result.roles = template.roles.clone();
    result.opacity = template.opacity;
    result.blend_mode = template.blend_mode;
    // Geometry is already in parent space, so the result carries no transform of its own.
    result.transform = Transform::IDENTITY;

    let mut inverse = Vec::new();

    // Remove operands back-to-front so earlier indices stay valid.
    let mut removals: Vec<(NodeLocation, NodeId)> =
        locs.iter().cloned().zip(ids.iter().cloned()).collect();
    removals.sort_by_key(|(l, _)| std::cmp::Reverse(l.index_in_parent().unwrap_or(0)));
    for (_, id) in &removals {
        let mut sub = delete_node(doc, id, report)?;
        inverse.append(&mut sub);
    }

    let mut sub = insert_node(doc, &parent_id, Some(insert_index), &result, report)?;

    // Undo means: drop the result first, then put the operands back. The operands were
    // removed highest-index-first, so their inserts have to run in the opposite order —
    // re-inserting at index 1 before index 0 exists would be out of range.
    inverse.reverse();
    sub.append(&mut inverse);
    Ok(sub)
}

fn parent_and_index(loc: &NodeLocation) -> Result<(NodeLocation, usize)> {
    let parent = loc.parent().ok_or(DocError::RootIsImmovable)?;
    let index = loc.index_in_parent().ok_or(DocError::RootIsImmovable)?;
    Ok((parent, index))
}

// ---------------------------------------------------------------------------
// Page, timeline and token operations
// ---------------------------------------------------------------------------

fn insert_page(doc: &mut Document, page: &Page, index: Option<usize>) -> Result<Vec<Op>> {
    ensure_finite(page)?;
    validate_page_timelines(page)?;
    if doc.page(page.id.as_str()).is_some() || doc.page(&page.slug).is_some() {
        return Err(DocError::InvalidValue {
            path: "page".into(),
            reason: format!("a page with id or slug '{}' already exists", page.slug),
        });
    }
    let len = doc.pages.len();
    let at = index.unwrap_or(len);
    if at > len {
        return Err(DocError::IndexOutOfRange { index: at, len });
    }
    doc.pages.insert(at, page.clone());
    Ok(vec![Op::PageDelete {
        page: page.id.as_str().to_string(),
    }])
}

fn delete_page(doc: &mut Document, key: &str) -> Result<Vec<Op>> {
    if doc.pages.len() == 1 {
        return Err(DocError::InvalidValue {
            path: "page".into(),
            reason: "a project must keep at least one page".into(),
        });
    }
    let index = doc
        .page_index(key)
        .ok_or_else(|| DocError::PageNotFound(key.to_string()))?;
    let removed = doc.pages.remove(index);
    Ok(vec![Op::PageInsert {
        page: Box::new(removed),
        index: Some(index),
    }])
}

fn update_page(doc: &mut Document, key: &str, path: &str, value: &Value) -> Result<Vec<Op>> {
    let index = doc
        .page_index(key)
        .ok_or_else(|| DocError::PageNotFound(key.to_string()))?;

    if path == "root" || path.starts_with("root.") {
        return Err(DocError::InvalidValue {
            path: path.to_string(),
            reason: "edit the scene graph with node.* operations, not page.update".into(),
        });
    }

    let mut tree = serde_json::to_value(&doc.pages[index])?;
    let previous = set_path(&mut tree, path, value.clone())?;
    let updated: Page = serde_json::from_value(tree).map_err(|e| DocError::InvalidValue {
        path: path.to_string(),
        reason: e.to_string(),
    })?;

    // Only when the edit could have reached them: a project that already carries a
    // malformed track predates this check and should still be editable in every other
    // respect.
    if path == "timelines" || path.starts_with("timelines.") {
        validate_page_timelines(&updated)?;
    }

    doc.pages[index] = updated;
    Ok(vec![Op::PageUpdate {
        page: key.to_string(),
        path: path.to_string(),
        value: previous.unwrap_or(Value::Null),
    }])
}

/// Refuse keyframes that [`crate::anim::Track::sample`] could not read.
///
/// Sampling assumes times are normalized and ascending, and nothing until now checked:
/// an unsorted or out-of-range track samples to the wrong value in the studio and
/// exports offsets the browser will not accept, both silently. A track with no keyframes
/// at all is fine — that is an animation package saying it has nothing to bake yet.
///
/// `prefix` places the reported path inside whatever delivered the timeline, which is
/// not always `timeline.set`.
fn validate_keyframes(timeline: &Timeline, prefix: &str) -> Result<()> {
    for (ti, track) in timeline.tracks.iter().enumerate() {
        let mut previous: Option<f64> = None;
        for (ki, frame) in track.keyframes.iter().enumerate() {
            let at = || format!("{prefix}tracks.{ti}.keyframes.{ki}.t");
            if !frame.t.is_finite() {
                return Err(DocError::InvalidValue {
                    path: at(),
                    reason: format!(
                        "keyframe time on '{}' is not a finite number",
                        track.property
                    ),
                });
            }
            if !(0.0..=1.0).contains(&frame.t) {
                return Err(DocError::InvalidValue {
                    path: at(),
                    reason: format!(
                        "keyframe time on '{}' is {}; times are normalized to 0..=1",
                        track.property, frame.t
                    ),
                });
            }
            if let Some(prev) = previous {
                if frame.t < prev {
                    return Err(DocError::InvalidValue {
                        path: at(),
                        reason: format!(
                            "keyframes on '{}' must be in ascending order; {} follows {prev}",
                            track.property, frame.t
                        ),
                    });
                }
            }
            previous = Some(frame.t);
        }
    }
    Ok(())
}

/// The same check for the timelines a whole page carries.
///
/// A timeline reaches a page by three doors, not one: `timeline.set`, a `page.insert`
/// whose page already has timelines on it, and a `page.update` writing the `timelines`
/// property wholesale. Guarding only the first leaves the other two as ways to store a
/// track that [`crate::anim::Track::sample`] cannot read.
fn validate_page_timelines(page: &Page) -> Result<()> {
    for (i, timeline) in page.timelines.iter().enumerate() {
        validate_keyframes(timeline, &format!("timelines.{i}."))?;
    }
    Ok(())
}

fn set_timeline(doc: &mut Document, key: &str, timeline: &Timeline) -> Result<Vec<Op>> {
    validate_keyframes(timeline, "")?;
    ensure_finite(timeline)?;

    let index = doc
        .page_index(key)
        .ok_or_else(|| DocError::PageNotFound(key.to_string()))?;
    let page = &mut doc.pages[index];

    match page.timelines.iter().position(|t| t.id == timeline.id) {
        Some(at) => {
            let previous = std::mem::replace(&mut page.timelines[at], timeline.clone());
            Ok(vec![Op::TimelineSet {
                page: key.to_string(),
                timeline: Box::new(previous),
            }])
        }
        None => {
            page.timelines.push(timeline.clone());
            Ok(vec![Op::TimelineRemove {
                page: key.to_string(),
                id: timeline.id.clone(),
            }])
        }
    }
}

fn remove_timeline(doc: &mut Document, key: &str, id: &TimelineId) -> Result<Vec<Op>> {
    let index = doc
        .page_index(key)
        .ok_or_else(|| DocError::PageNotFound(key.to_string()))?;
    let page = &mut doc.pages[index];
    let at = page
        .timelines
        .iter()
        .position(|t| &t.id == id)
        .ok_or_else(|| DocError::InvalidValue {
            path: "id".into(),
            reason: format!("no timeline {id} on page '{key}'"),
        })?;
    let removed = page.timelines.remove(at);
    Ok(vec![Op::TimelineSet {
        page: key.to_string(),
        timeline: Box::new(removed),
    }])
}

fn set_tokens(doc: &mut Document, path: &str, value: &Value) -> Result<Vec<Op>> {
    let mut tree = serde_json::to_value(&doc.tokens)?;
    // Token groups are frequently empty, and asking a model to create the container
    // before writing into it is friction with no upside — the shapes are fixed and
    // known, so an absent group is unambiguous.
    if let Value::Object(map) = &mut tree {
        for group in ["colors", "fonts", "spacing"] {
            map.entry(group.to_string())
                .or_insert_with(|| Value::Object(Default::default()));
        }
    }

    let previous = set_path(&mut tree, path, value.clone())?;
    let updated: crate::document::Tokens =
        serde_json::from_value(tree).map_err(|e| DocError::InvalidValue {
            path: path.to_string(),
            reason: e.to_string(),
        })?;

    doc.tokens = updated;
    Ok(vec![Op::TokensSet {
        path: path.to_string(),
        value: previous.unwrap_or(Value::Null),
    }])
}

// ---------------------------------------------------------------------------
// Dotted-path access
// ---------------------------------------------------------------------------

/// Set `path` within `tree`, returning what was there before.
///
/// `None` means the key did not exist. A `Value::Null` new value removes the key, which
/// is how an optional property gets cleared and how "restore a property that was not
/// there" works when undoing.
///
/// Intermediate containers are never created. That strictness is on purpose: a typo
/// like `fil1s.0.color` should be an error a model can see and correct, not a stray key
/// that silently does nothing.
fn set_path(tree: &mut Value, path: &str, value: Value) -> Result<Option<Value>> {
    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(DocError::NoSuchProperty(path.to_string()));
    }

    let mut cursor = tree;
    for seg in &segments[..segments.len() - 1] {
        cursor = descend(cursor, seg, path)?;
    }

    let last = segments[segments.len() - 1];
    match cursor {
        Value::Object(map) => {
            if value.is_null() {
                Ok(map.remove(last))
            } else {
                Ok(map.insert(last.to_string(), value))
            }
        }
        Value::Array(arr) => {
            let i: usize = last
                .parse()
                .map_err(|_| DocError::NoSuchProperty(path.to_string()))?;
            if i >= arr.len() {
                return Err(DocError::IndexOutOfRange {
                    index: i,
                    len: arr.len(),
                });
            }
            Ok(Some(std::mem::replace(&mut arr[i], value)))
        }
        _ => Err(DocError::NoSuchProperty(path.to_string())),
    }
}

fn descend<'a>(cursor: &'a mut Value, seg: &str, full: &str) -> Result<&'a mut Value> {
    match cursor {
        Value::Object(map) => map
            .get_mut(seg)
            .ok_or_else(|| DocError::NoSuchProperty(full.to_string())),
        Value::Array(arr) => {
            let i: usize = seg
                .parse()
                .map_err(|_| DocError::NoSuchProperty(full.to_string()))?;
            let len = arr.len();
            arr.get_mut(i)
                .ok_or(DocError::IndexOutOfRange { index: i, len })
        }
        _ => Err(DocError::NoSuchProperty(full.to_string())),
    }
}

/// Read a dotted path, for callers that want to inspect before they write.
pub fn get_path<'a>(tree: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = tree;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cursor = match cursor {
            Value::Object(map) => map.get(seg)?,
            Value::Array(arr) => arr.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cursor)
}

/// Convenience for `PageId`-typed callers.
pub fn page_key(id: &PageId) -> String {
    id.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{Easing, Keyframe, Track, Trigger};
    use crate::node::RectGeometry;

    fn doc() -> Document {
        let mut d = Document::new("Site");
        d.pages[0].root.id = NodeId::from_static("nd_root");
        d
    }

    fn frame(t: f64) -> Keyframe {
        Keyframe {
            t,
            value: Value::from(1.0),
            easing: Easing::Linear,
        }
    }

    fn timeline(keyframes: Vec<Keyframe>) -> Timeline {
        Timeline {
            id: TimelineId::from_static("tl_a"),
            name: String::new(),
            trigger: Trigger::Load { delay: 0.0 },
            duration: 1.0,
            enabled: true,
            reduced_motion: Default::default(),
            source: None,
            tracks: vec![Track {
                target: NodeId::from_static("nd_root"),
                property: "opacity".into(),
                keyframes,
            }],
        }
    }

    fn set_keyframes(d: &mut Document, keyframes: Vec<Keyframe>) -> Result<()> {
        apply_ops(
            d,
            &[Op::TimelineSet {
                page: "index".into(),
                timeline: Box::new(timeline(keyframes)),
            }],
        )
        .map(|_| ())
    }

    fn rect(width: f64) -> Node {
        Node::new(
            NodeId::from_static("nd_card"),
            NodeKind::Rect(RectGeometry {
                width,
                height: 10.0,
                corner_radius: [0.0; 4],
            }),
        )
    }

    #[test]
    fn keyframes_outside_the_unit_interval_are_refused() {
        let mut d = doc();
        let err = set_keyframes(&mut d, vec![frame(0.0), frame(1.5)])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("opacity") && err.contains("0..=1"),
            "got {err}"
        );
        assert!(d.pages[0].timelines.is_empty(), "a refused op still landed");

        assert!(set_keyframes(&mut d, vec![frame(-0.1), frame(1.0)]).is_err());
    }

    #[test]
    fn keyframes_out_of_order_are_refused() {
        let mut d = doc();
        let err = set_keyframes(&mut d, vec![frame(0.0), frame(0.8), frame(0.4)])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("opacity") && err.contains("ascending"),
            "got {err}"
        );
    }

    #[test]
    fn non_finite_keyframe_times_are_refused() {
        let mut d = doc();
        let err = set_keyframes(&mut d, vec![frame(0.0), frame(f64::NAN)])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("opacity") && err.contains("finite"),
            "got {err}"
        );
    }

    #[test]
    fn well_formed_keyframes_are_accepted() {
        let mut d = doc();
        // No keyframes at all is a package that has not baked anything yet, and a
        // repeated time is how a hard cut is spelled.
        set_keyframes(&mut d, Vec::new()).unwrap();
        set_keyframes(&mut d, vec![frame(0.0), frame(0.5), frame(0.5), frame(1.0)]).unwrap();
        assert_eq!(d.pages[0].timelines.len(), 1);
    }

    #[test]
    fn a_page_cannot_arrive_carrying_keyframes_timeline_set_would_refuse() {
        let mut d = doc();
        let mut page = Page::new(PageId::from_static("pg_b"), "About", "about", 1440.0, 900.0);
        page.timelines.push(timeline(vec![frame(0.9), frame(0.1)]));
        let err = apply_ops(
            &mut d,
            &[Op::PageInsert {
                page: Box::new(page),
                index: None,
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("ascending"), "got {err}");
        assert_eq!(d.pages.len(), 1);
    }

    #[test]
    fn page_update_cannot_write_keyframes_around_the_check() {
        let mut d = doc();
        let smuggled = serde_json::to_value(timeline(vec![frame(0.0), frame(9.0)])).unwrap();
        let err = apply_ops(
            &mut d,
            &[Op::PageUpdate {
                page: "index".into(),
                path: "timelines".into(),
                value: Value::Array(vec![smuggled]),
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("0..=1"), "got {err}");
        assert!(d.pages[0].timelines.is_empty(), "a refused op still landed");
    }

    #[test]
    fn a_non_finite_size_cannot_be_inserted() {
        let mut d = doc();
        let err = apply_ops(
            &mut d,
            &[Op::NodeInsert {
                parent: NodeId::from_static("nd_root"),
                index: None,
                node: rect(f64::NAN),
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("width"), "got {err}");
        assert!(d.pages[0].root.children.is_empty());

        apply_ops(
            &mut d,
            &[Op::NodeInsert {
                parent: NodeId::from_static("nd_root"),
                index: None,
                node: rect(10.0),
            }],
        )
        .unwrap();
    }

    #[test]
    fn a_non_finite_page_size_cannot_be_inserted() {
        let mut d = doc();
        let mut page = Page::new(PageId::from_static("pg_b"), "About", "about", 1440.0, 900.0);
        page.width = f64::INFINITY;
        let err = apply_ops(
            &mut d,
            &[Op::PageInsert {
                page: Box::new(page),
                index: None,
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("width"), "got {err}");
        assert_eq!(d.pages.len(), 1);
    }
}
