//! Undo, redo, and the editing session that owns them.
//!
//! There is one history for the whole document, shared by every source of edits. An AI
//! patch that came in over MCP sits on the same stack as a mouse drag and is undone with
//! the same keystroke. That is not a convenience — it is the property that makes the AI
//! feel like a collaborator rather than a code generator you have to clean up after. If
//! a model's change were not undoable, no one would let it touch anything.
//!
//! Undo stores *inverse operations*, not document snapshots. Snapshots would be simpler
//! but a hundred deep on a large document would cost hundreds of megabytes, which is not
//! available on the phone this has to run on.

use crate::document::Document;
use crate::error::{DocError, Result};
use crate::patch::{apply_ops, Op, PatchReport};

/// How many undo steps to keep. Deep enough to cover a working session, bounded so a
/// long session cannot grow without limit.
pub const DEFAULT_HISTORY_LIMIT: usize = 200;

/// One reversible step, as it sits on a stack.
#[derive(Debug, Clone)]
pub struct Transaction {
    /// What to show in the Undo menu: "Move 3 layers", "Apply Stagger Fade Up".
    pub label: String,
    /// Ops that carry the document across this step in the direction of this stack.
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        History::with_limit(DEFAULT_HISTORY_LIMIT)
    }
}

impl History {
    pub fn with_limit(limit: usize) -> Self {
        History { undo: Vec::new(), redo: Vec::new(), limit: limit.max(1) }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    fn push_undo(&mut self, tx: Transaction) {
        self.undo.push(tx);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }
}

/// A document plus its history — what an open editor window holds.
#[derive(Debug, Clone)]
pub struct Session {
    doc: Document,
    history: History,
    /// Incremented on every accepted change, so views and the MCP server can tell
    /// cheaply whether anything moved since they last looked.
    revision: u64,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Session { doc, history: History::default(), revision: 0 }
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Direct mutable access, bypassing history.
    ///
    /// For loading and for tests. Editing through this leaves the undo stack describing
    /// a document that no longer exists, so anything user-facing should go through
    /// [`Session::apply`].
    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.doc
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Apply ops as one undoable step.
    ///
    /// A patch that fails changes nothing — not the document, not the history.
    pub fn apply(&mut self, label: impl Into<String>, ops: Vec<Op>) -> Result<PatchReport> {
        let (inverse, report) = apply_ops(&mut self.doc, &ops)?;
        self.history.push_undo(Transaction { label: label.into(), ops: inverse });
        // A new edit invalidates any future that was branched away from.
        self.history.redo.clear();
        self.revision += 1;
        Ok(report)
    }

    pub fn undo(&mut self) -> Result<PatchReport> {
        let tx = self.history.undo.pop().ok_or(DocError::NothingToUndo)?;
        let (inverse, report) = match apply_ops(&mut self.doc, &tx.ops) {
            Ok(v) => v,
            Err(e) => {
                // Put it back: a failed undo must not silently consume the step.
                self.history.undo.push(tx);
                return Err(e);
            }
        };
        self.history.redo.push(Transaction { label: tx.label, ops: inverse });
        self.revision += 1;
        Ok(report)
    }

    pub fn redo(&mut self) -> Result<PatchReport> {
        let tx = self.history.redo.pop().ok_or(DocError::NothingToRedo)?;
        let (inverse, report) = match apply_ops(&mut self.doc, &tx.ops) {
            Ok(v) => v,
            Err(e) => {
                self.history.redo.push(tx);
                return Err(e);
            }
        };
        self.history.push_undo(Transaction { label: tx.label, ops: inverse });
        self.revision += 1;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::to_canonical_string;
    use crate::id::NodeId;
    use crate::node::{Node, NodeKind, RectGeometry};
    use serde_json::json;

    fn rect(id: &'static str) -> Node {
        Node::new(
            NodeId::from_static(id),
            NodeKind::Rect(RectGeometry { width: 10.0, height: 10.0, corner_radius: [0.0; 4] }),
        )
    }

    fn session() -> (Session, NodeId) {
        let doc = Document::new("Test");
        let root = doc.pages[0].root.id.clone();
        (Session::new(doc), root)
    }

    #[test]
    fn apply_then_undo_restores_the_exact_bytes() {
        let (mut s, root) = session();
        let before = to_canonical_string(s.document()).unwrap();

        s.apply(
            "Add rectangle",
            vec![Op::NodeInsert { parent: root, index: None, node: rect("nd_1") }],
        )
        .unwrap();
        assert_ne!(to_canonical_string(s.document()).unwrap(), before);

        s.undo().unwrap();
        assert_eq!(
            to_canonical_string(s.document()).unwrap(),
            before,
            "undo did not return the document to its previous state"
        );
    }

    #[test]
    fn redo_reapplies_what_undo_took_away() {
        let (mut s, root) = session();
        s.apply(
            "Add rectangle",
            vec![Op::NodeInsert { parent: root, index: None, node: rect("nd_1") }],
        )
        .unwrap();
        let after = to_canonical_string(s.document()).unwrap();

        s.undo().unwrap();
        s.redo().unwrap();
        assert_eq!(to_canonical_string(s.document()).unwrap(), after);
    }

    #[test]
    fn an_ai_patch_and_a_gui_edit_share_one_stack() {
        // The property the whole design rests on: whoever made the change, one Ctrl+Z
        // takes it back, and the stack interleaves them in the order they happened.
        let (mut s, root) = session();
        let baseline = to_canonical_string(s.document()).unwrap();

        s.apply(
            "AI: add hero",
            vec![Op::NodeInsert { parent: root.clone(), index: None, node: rect("nd_ai") }],
        )
        .unwrap();
        let after_ai = to_canonical_string(s.document()).unwrap();

        s.apply(
            "Drag: resize",
            vec![Op::NodeUpdate {
                id: NodeId::from_static("nd_ai"),
                path: "width".into(),
                value: json!(250.0),
            }],
        )
        .unwrap();

        s.undo().unwrap();
        assert_eq!(to_canonical_string(s.document()).unwrap(), after_ai, "GUI edit did not undo");
        s.undo().unwrap();
        assert_eq!(to_canonical_string(s.document()).unwrap(), baseline, "AI edit did not undo");
    }

    #[test]
    fn a_new_edit_discards_the_redo_branch() {
        let (mut s, root) = session();
        s.apply("A", vec![Op::NodeInsert { parent: root.clone(), index: None, node: rect("nd_1") }])
            .unwrap();
        s.undo().unwrap();
        assert!(s.history().can_redo());

        s.apply("B", vec![Op::NodeInsert { parent: root, index: None, node: rect("nd_2") }])
            .unwrap();
        assert!(!s.history().can_redo(), "redo should not survive a divergent edit");
    }

    #[test]
    fn a_failed_patch_leaves_document_and_history_untouched() {
        let (mut s, root) = session();
        let before = to_canonical_string(s.document()).unwrap();

        let err = s.apply(
            "Bad patch",
            vec![
                Op::NodeInsert { parent: root, index: None, node: rect("nd_ok") },
                Op::NodeUpdate {
                    id: NodeId::from_static("nd_missing"),
                    path: "width".into(),
                    value: json!(10.0),
                },
            ],
        );

        assert!(err.is_err());
        assert_eq!(
            to_canonical_string(s.document()).unwrap(),
            before,
            "the first op of a failed patch was left applied"
        );
        assert!(!s.history().can_undo(), "a failed patch should not create an undo step");
    }

    #[test]
    fn nothing_to_undo_is_an_error_not_a_panic() {
        let (mut s, _) = session();
        assert!(s.undo().is_err());
        assert!(s.redo().is_err());
    }

    #[test]
    fn the_stack_is_bounded() {
        let doc = Document::new("Test");
        let root = doc.pages[0].root.id.clone();
        let mut s = Session { doc, history: History::with_limit(3), revision: 0 };

        for i in 0..10 {
            let mut n = rect("nd_x");
            n.id = NodeId::parse(format!("nd_{i}")).unwrap();
            s.apply("step", vec![Op::NodeInsert { parent: root.clone(), index: None, node: n }])
                .unwrap();
        }
        assert_eq!(s.history().depth().0, 3);
    }

    #[test]
    fn labels_surface_for_the_undo_menu() {
        let (mut s, root) = session();
        s.apply(
            "Add rectangle",
            vec![Op::NodeInsert { parent: root, index: None, node: rect("nd_1") }],
        )
        .unwrap();
        assert_eq!(s.history().undo_label(), Some("Add rectangle"));
        s.undo().unwrap();
        assert_eq!(s.history().redo_label(), Some("Add rectangle"));
    }

    #[test]
    fn revision_advances_on_every_accepted_change() {
        let (mut s, root) = session();
        assert_eq!(s.revision(), 0);
        s.apply("A", vec![Op::NodeInsert { parent: root, index: None, node: rect("nd_1") }])
            .unwrap();
        assert_eq!(s.revision(), 1);
        s.undo().unwrap();
        assert_eq!(s.revision(), 2);
    }
}
