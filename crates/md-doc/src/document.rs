//! Projects, pages and the document as a whole.

use crate::anim::Timeline;
use crate::error::{DocError, Result};
use crate::id::{NodeId, PageId};
use crate::node::{FrameGeometry, Node, NodeKind};
use crate::paint::{Color, Paint};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bumped whenever the format changes in a way older readers cannot handle.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub schema_version: u32,
    pub meta: ProjectMeta,
    #[serde(default, skip_serializing_if = "Tokens::is_empty")]
    pub tokens: Tokens,
    pub pages: Vec<Page>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Responsive breakpoints, narrowest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breakpoints: Vec<Breakpoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    pub name: String,
    pub min_width: f64,
}

impl Default for ProjectMeta {
    fn default() -> Self {
        ProjectMeta {
            name: "Untitled".into(),
            description: String::new(),
            // Phone, tablet, desktop. The same three shapes the studio's own UI adapts
            // to, which keeps the tool and the thing it designs speaking one language.
            breakpoints: vec![
                Breakpoint {
                    name: "sm".into(),
                    min_width: 0.0,
                },
                Breakpoint {
                    name: "md".into(),
                    min_width: 768.0,
                },
                Breakpoint {
                    name: "lg".into(),
                    min_width: 1280.0,
                },
            ],
        }
    }
}

/// Named design values shared across the project.
///
/// Tokens are what make an AI edit like "warm up the accent colour" a one-line change
/// instead of a hunt through every node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, Color>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fonts: BTreeMap<String, FontToken>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spacing: BTreeMap<String, f64>,
}

impl Tokens {
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty() && self.fonts.is_empty() && self.spacing.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontToken {
    pub family: String,
    pub size: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub id: PageId,
    pub name: String,
    /// URL path segment. `index` becomes the site root.
    pub slug: String,
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Paint>,
    pub root: Node,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timelines: Vec<Timeline>,
}

/// Where a node sits: which page, and the child indices to walk from that page's root.
///
/// Positions rather than references, so callers can look something up and then mutate
/// without fighting the borrow checker — which the patch protocol does constantly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLocation {
    pub page: usize,
    /// Empty for the page root.
    pub path: Vec<usize>,
}

impl NodeLocation {
    pub fn is_root(&self) -> bool {
        self.path.is_empty()
    }

    /// Location of this node's parent, or `None` for a page root.
    pub fn parent(&self) -> Option<NodeLocation> {
        if self.path.is_empty() {
            return None;
        }
        let mut path = self.path.clone();
        path.pop();
        Some(NodeLocation {
            page: self.page,
            path,
        })
    }

    pub fn index_in_parent(&self) -> Option<usize> {
        self.path.last().copied()
    }
}

impl Page {
    pub fn new(
        id: PageId,
        name: impl Into<String>,
        slug: impl Into<String>,
        width: f64,
        height: f64,
    ) -> Self {
        let name = name.into();
        let root = Node::new(
            NodeId::new(),
            NodeKind::Frame(FrameGeometry {
                width,
                height,
                clip: true,
                corner_radius: [0.0; 4],
                layout: None,
            }),
        )
        .with_name(name.clone());

        Page {
            id,
            name,
            slug: slug.into(),
            width,
            height,
            background: None,
            root,
            timelines: Vec::new(),
        }
    }

    /// Filename this page exports to.
    pub fn output_file(&self) -> String {
        if self.slug == "index" || self.slug.is_empty() {
            "index.html".to_string()
        } else {
            format!("{}/index.html", self.slug)
        }
    }
}

impl Document {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Document {
            schema_version: SCHEMA_VERSION,
            meta: ProjectMeta {
                name,
                ..ProjectMeta::default()
            },
            tokens: Tokens::default(),
            pages: vec![Page::new(PageId::new(), "Home", "index", 1440.0, 900.0)],
        }
    }

    /// A new document whose first page is a given size, on a white ground.
    ///
    /// Both the CLI and the studio create projects, and both were setting the page size,
    /// resizing the root frame to match, and choosing a background separately. Missing
    /// the root-frame half leaves a page whose artboard and whose content frame disagree
    /// — everything lays out against 1440×900 no matter what the page says — and that is
    /// exactly the kind of bug that appears in one entry point and not the other.
    pub fn sized(name: impl Into<String>, width: f64, height: f64) -> Result<Self> {
        let mut doc = Document::new(name);
        doc.pages[0].width = width;
        doc.pages[0].height = height;
        if let NodeKind::Frame(frame) = &mut doc.pages[0].root.kind {
            frame.width = width;
            frame.height = height;
        }
        doc.pages[0].background = Some(Paint::solid("#ffffff")?);
        Ok(doc)
    }

    /// Look up a page by id or by slug — whichever the caller happens to have.
    pub fn page(&self, key: &str) -> Option<&Page> {
        self.pages
            .iter()
            .find(|p| p.id.as_str() == key || p.slug == key)
    }

    pub fn page_mut(&mut self, key: &str) -> Option<&mut Page> {
        self.pages
            .iter_mut()
            .find(|p| p.id.as_str() == key || p.slug == key)
    }

    pub fn page_index(&self, key: &str) -> Option<usize> {
        self.pages
            .iter()
            .position(|p| p.id.as_str() == key || p.slug == key)
    }

    /// Find where a node lives. `O(n)` over the document, which is microseconds at any
    /// document size a person will build by hand.
    pub fn locate(&self, id: &NodeId) -> Option<NodeLocation> {
        for (page_index, page) in self.pages.iter().enumerate() {
            let mut path = Vec::new();
            if locate_in(&page.root, id, &mut path) {
                return Some(NodeLocation {
                    page: page_index,
                    path,
                });
            }
        }
        None
    }

    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        let loc = self.locate(id)?;
        self.node_at(&loc)
    }

    pub fn node_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        let loc = self.locate(id)?;
        self.node_at_mut(&loc)
    }

    pub fn node_at(&self, loc: &NodeLocation) -> Option<&Node> {
        let mut node = &self.pages.get(loc.page)?.root;
        for i in &loc.path {
            node = node.children.get(*i)?;
        }
        Some(node)
    }

    pub fn node_at_mut(&mut self, loc: &NodeLocation) -> Option<&mut Node> {
        let mut node = &mut self.pages.get_mut(loc.page)?.root;
        for i in &loc.path {
            node = node.children.get_mut(*i)?;
        }
        Some(node)
    }

    pub fn require_node(&self, id: &NodeId) -> Result<&Node> {
        self.node(id)
            .ok_or_else(|| DocError::NodeNotFound(id.clone()))
    }

    pub fn parent_of(&self, id: &NodeId) -> Option<&Node> {
        let loc = self.locate(id)?;
        self.node_at(&loc.parent()?)
    }

    /// Every node in the document, in document order.
    pub fn walk(&self, f: &mut impl FnMut(&Page, &Node)) {
        for page in &self.pages {
            page.root.walk(&mut |n| f(page, n));
        }
    }

    pub fn node_count(&self) -> usize {
        let mut n = 0;
        self.walk(&mut |_, _| n += 1);
        n
    }

    /// Every distinct role used in the document, with how many nodes carry it.
    ///
    /// This is the vocabulary an animation or an AI instruction can address, so it is
    /// the first thing worth showing a model that is orienting itself.
    pub fn role_index(&self) -> BTreeMap<String, usize> {
        let mut out: BTreeMap<String, usize> = BTreeMap::new();
        self.walk(&mut |_, n| {
            for r in &n.roles {
                *out.entry(r.clone()).or_insert(0) += 1;
            }
        });
        out
    }

    /// Ids that appear more than once.
    ///
    /// Duplicates would make the patch protocol ambiguous — `node.update` would not know
    /// which node it addressed — so loading validates against this.
    pub fn duplicate_ids(&self) -> Vec<NodeId> {
        let mut seen: BTreeMap<NodeId, usize> = BTreeMap::new();
        self.walk(&mut |_, n| *seen.entry(n.id.clone()).or_insert(0) += 1);
        seen.into_iter()
            .filter(|(_, c)| *c > 1)
            .map(|(id, _)| id)
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        let dupes = self.duplicate_ids();
        if let Some(first) = dupes.first() {
            return Err(DocError::DuplicateNode(first.clone()));
        }
        for page in &self.pages {
            let mut bad: Option<NodeId> = None;
            page.root.walk(&mut |n| {
                if bad.is_none() && !n.children.is_empty() && !n.is_container() {
                    bad = Some(n.id.clone());
                }
            });
            if let Some(id) = bad {
                return Err(DocError::NotAContainer(id));
            }
        }
        Ok(())
    }

    pub fn timeline_count(&self) -> usize {
        self.pages.iter().map(|p| p.timelines.len()).sum()
    }
}

fn locate_in(node: &Node, id: &NodeId, path: &mut Vec<usize>) -> bool {
    if &node.id == id {
        return true;
    }
    for (i, child) in node.children.iter().enumerate() {
        path.push(i);
        if locate_in(child, id, path) {
            return true;
        }
        path.pop();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::RectGeometry;

    fn rect(id: &'static str) -> Node {
        Node::new(
            NodeId::from_static(id),
            NodeKind::Rect(RectGeometry {
                width: 10.0,
                height: 10.0,
                corner_radius: [0.0; 4],
            }),
        )
    }

    fn doc_with_tree() -> Document {
        let mut doc = Document::new("Test");
        let group = Node::new(NodeId::from_static("nd_group"), NodeKind::Group)
            .with_children(vec![rect("nd_a"), rect("nd_b")]);
        doc.pages[0].root.children.push(group);
        doc.pages[0].root.id = NodeId::from_static("nd_root");
        doc
    }

    #[test]
    fn a_new_document_has_one_page_that_validates() {
        let doc = Document::new("My site");
        assert_eq!(doc.pages.len(), 1);
        assert_eq!(doc.pages[0].slug, "index");
        doc.validate().unwrap();
    }

    #[test]
    fn pages_resolve_by_slug_or_id() {
        let doc = Document::new("x");
        let id = doc.pages[0].id.as_str().to_string();
        assert!(doc.page("index").is_some());
        assert!(doc.page(&id).is_some());
        assert!(doc.page("nope").is_none());
    }

    #[test]
    fn locate_finds_the_path_to_a_nested_node() {
        let doc = doc_with_tree();
        let loc = doc.locate(&NodeId::from_static("nd_b")).unwrap();
        assert_eq!(loc.page, 0);
        assert_eq!(loc.path, vec![0, 1]);
        assert_eq!(doc.node_at(&loc).unwrap().id.as_str(), "nd_b");
    }

    #[test]
    fn the_page_root_locates_to_an_empty_path() {
        let doc = doc_with_tree();
        let loc = doc.locate(&NodeId::from_static("nd_root")).unwrap();
        assert!(loc.is_root());
        assert!(loc.parent().is_none());
    }

    #[test]
    fn parent_lookup_walks_up_one_level() {
        let doc = doc_with_tree();
        let parent = doc.parent_of(&NodeId::from_static("nd_a")).unwrap();
        assert_eq!(parent.id.as_str(), "nd_group");
    }

    #[test]
    fn missing_nodes_report_their_id() {
        let doc = doc_with_tree();
        let err = doc
            .require_node(&NodeId::from_static("nd_ghost"))
            .unwrap_err();
        assert!(err.to_string().contains("nd_ghost"), "got {err}");
    }

    #[test]
    fn duplicate_ids_fail_validation() {
        let mut doc = doc_with_tree();
        doc.pages[0].root.children.push(rect("nd_a"));
        assert_eq!(doc.duplicate_ids(), vec![NodeId::from_static("nd_a")]);
        assert!(doc.validate().is_err());
    }

    #[test]
    fn children_on_a_leaf_fail_validation() {
        let mut doc = doc_with_tree();
        let mut leaf = rect("nd_leaf");
        leaf.children.push(rect("nd_inner"));
        doc.pages[0].root.children.push(leaf);
        let err = doc.validate().unwrap_err();
        assert!(err.to_string().contains("nd_leaf"), "got {err}");
    }

    #[test]
    fn roles_are_indexed_with_counts() {
        let mut doc = doc_with_tree();
        doc.node_mut(&NodeId::from_static("nd_a"))
            .unwrap()
            .roles
            .push("card".into());
        doc.node_mut(&NodeId::from_static("nd_b"))
            .unwrap()
            .roles
            .push("card".into());
        let index = doc.role_index();
        assert_eq!(index.get("card"), Some(&2));
    }

    #[test]
    fn page_slugs_map_to_output_paths() {
        let mut p = Page::new(PageId::from_static("pg_1"), "Home", "index", 100.0, 100.0);
        assert_eq!(p.output_file(), "index.html");
        p.slug = "about".into();
        assert_eq!(p.output_file(), "about/index.html");
    }

    #[test]
    fn a_document_round_trips_through_canonical_json() {
        let doc = doc_with_tree();
        let text = crate::canonical::to_canonical_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&text).unwrap();
        assert_eq!(doc, back);
        assert_eq!(text, crate::canonical::to_canonical_string(&back).unwrap());
    }
}
