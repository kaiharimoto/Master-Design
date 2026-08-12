//! Selectors.
//!
//! A patch that says "every card" instead of listing five ids is shorter, survives the
//! document changing under it, and is what an animation package needs in order to be
//! reusable across projects at all. The syntax is deliberately CSS-shaped, because both
//! the people and the models using it already know CSS.
//!
//! ```text
//! #nd_01hq5…            a specific node
//! @card                 every node tagged with the role "card"
//! type:text             every text node
//! name:"Hero title"     by layer name
//! *                     everything
//! @section @card        cards anywhere inside a section  (descendant)
//! type:path@decorative  paths that are also tagged decorative  (compound)
//! @card, @cta           either  (union, document order, no duplicates)
//! ```

use crate::document::Document;
use crate::error::{DocError, Result};
use crate::id::NodeId;
use crate::node::Node;

/// A parsed selector: one or more comma-separated alternatives.
#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    source: String,
    groups: Vec<Group>,
}

/// A descendant chain — `@section @card` is two steps.
#[derive(Debug, Clone, PartialEq)]
struct Group(Vec<Compound>);

/// Conditions that must all hold on a single node.
#[derive(Debug, Clone, PartialEq)]
struct Compound(Vec<Term>);

#[derive(Debug, Clone, PartialEq)]
enum Term {
    Any,
    Id(String),
    Role(String),
    Type(String),
    Name(String),
}

impl Selector {
    pub fn parse(input: &str) -> Result<Selector> {
        let source = input.trim().to_string();
        if source.is_empty() {
            return Err(DocError::InvalidSelector {
                selector: input.to_string(),
                reason: "selector is empty".into(),
            });
        }

        let mut groups = Vec::new();
        for alternative in split_top_level(&source, ',') {
            let alternative = alternative.trim();
            if alternative.is_empty() {
                return Err(DocError::InvalidSelector {
                    selector: input.to_string(),
                    reason: "empty alternative between commas".into(),
                });
            }
            let mut steps = Vec::new();
            for step in split_top_level(alternative, ' ') {
                let step = step.trim();
                if step.is_empty() {
                    continue;
                }
                steps.push(parse_compound(step, input)?);
            }
            if steps.is_empty() {
                return Err(DocError::InvalidSelector {
                    selector: input.to_string(),
                    reason: "no terms".into(),
                });
            }
            groups.push(Group(steps));
        }

        Ok(Selector { source, groups })
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Every matching node in the document, in document order, without duplicates.
    pub fn select(&self, doc: &Document) -> Vec<NodeId> {
        let mut out = Vec::new();
        for page in &doc.pages {
            self.collect(&page.root, &mut Vec::new(), &mut out);
        }
        out
    }

    /// Matches within a single page.
    pub fn select_in_page(&self, doc: &Document, page_key: &str) -> Result<Vec<NodeId>> {
        let page =
            doc.page(page_key).ok_or_else(|| DocError::PageNotFound(page_key.to_string()))?;
        let mut out = Vec::new();
        self.collect(&page.root, &mut Vec::new(), &mut out);
        Ok(out)
    }

    /// Does this node match, given its ancestors (outermost first)?
    pub fn matches(&self, node: &Node, ancestors: &[&Node]) -> bool {
        self.groups.iter().any(|g| group_matches(g, node, ancestors))
    }

    fn collect<'a>(&self, node: &'a Node, ancestors: &mut Vec<&'a Node>, out: &mut Vec<NodeId>) {
        if self.matches(node, ancestors) && !out.contains(&node.id) {
            out.push(node.id.clone());
        }
        ancestors.push(node);
        for child in &node.children {
            self.collect(child, ancestors, out);
        }
        ancestors.pop();
    }
}

fn group_matches(group: &Group, node: &Node, ancestors: &[&Node]) -> bool {
    let steps = &group.0;
    let last = match steps.last() {
        Some(l) => l,
        None => return false,
    };
    if !compound_matches(last, node) {
        return false;
    }

    // Walk the remaining steps backwards through the ancestor chain. Each must be found
    // somewhere above the previous one, which is what makes the combinator "descendant"
    // rather than "direct child".
    let mut remaining = &steps[..steps.len() - 1];
    let mut i = ancestors.len();
    while let Some(step) = remaining.last() {
        let mut found = false;
        while i > 0 {
            i -= 1;
            if compound_matches(step, ancestors[i]) {
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
        remaining = &remaining[..remaining.len() - 1];
    }
    true
}

fn compound_matches(compound: &Compound, node: &Node) -> bool {
    compound.0.iter().all(|t| term_matches(t, node))
}

fn term_matches(term: &Term, node: &Node) -> bool {
    match term {
        Term::Any => true,
        Term::Id(id) => node.id.as_str() == id,
        Term::Role(role) => node.has_role(role),
        Term::Type(ty) => node.kind.type_name() == ty,
        Term::Name(name) => node.name == *name,
    }
}

fn parse_compound(step: &str, full: &str) -> Result<Compound> {
    let mut terms = Vec::new();
    let chars: Vec<char> = step.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '*' => {
                terms.push(Term::Any);
                i += 1;
            }
            '#' => {
                let (val, next) = read_until_term_boundary(&chars, i + 1);
                if val.is_empty() {
                    return Err(bad(full, "'#' with no id"));
                }
                terms.push(Term::Id(val));
                i = next;
            }
            '@' => {
                let (val, next) = read_until_term_boundary(&chars, i + 1);
                if val.is_empty() {
                    return Err(bad(full, "'@' with no role"));
                }
                terms.push(Term::Role(val));
                i = next;
            }
            _ => {
                // `type:` and `name:` prefixes.
                let rest: String = chars[i..].iter().collect();
                if let Some(after) = rest.strip_prefix("type:") {
                    let start = i + "type:".len();
                    let (val, next) = read_until_term_boundary(&chars, start);
                    let _ = after;
                    if val.is_empty() {
                        return Err(bad(full, "'type:' with no value"));
                    }
                    terms.push(Term::Type(val));
                    i = next;
                } else if rest.starts_with("name:") {
                    let start = i + "name:".len();
                    let (val, next) = read_value(&chars, start, full)?;
                    terms.push(Term::Name(val));
                    i = next;
                } else {
                    return Err(bad(full, &format!("unexpected '{}'", chars[i])));
                }
            }
        }
    }

    if terms.is_empty() {
        return Err(bad(full, "empty step"));
    }
    Ok(Compound(terms))
}

/// Read a bare value up to the next term marker.
fn read_until_term_boundary(chars: &[char], start: usize) -> (String, usize) {
    let mut i = start;
    let mut out = String::new();
    while i < chars.len() && !matches!(chars[i], '#' | '@' | '*') {
        out.push(chars[i]);
        i += 1;
    }
    (out, i)
}

/// Read a value that may be quoted, so layer names can contain spaces and punctuation.
fn read_value(chars: &[char], start: usize, full: &str) -> Result<(String, usize)> {
    if start < chars.len() && (chars[start] == '"' || chars[start] == '\'') {
        let quote = chars[start];
        let mut i = start + 1;
        let mut out = String::new();
        while i < chars.len() && chars[i] != quote {
            out.push(chars[i]);
            i += 1;
        }
        if i >= chars.len() {
            return Err(bad(full, "unterminated quoted value"));
        }
        Ok((out, i + 1))
    } else {
        let (v, next) = read_until_term_boundary(chars, start);
        if v.is_empty() {
            return Err(bad(full, "missing value"));
        }
        Ok((v, next))
    }
}

/// Split on a separator, ignoring separators inside quotes.
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for c in s.chars() {
        match quote {
            Some(q) => {
                current.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                    current.push(c);
                } else if c == sep {
                    out.push(std::mem::take(&mut current));
                } else {
                    current.push(c);
                }
            }
        }
    }
    out.push(current);
    out
}

fn bad(selector: &str, reason: &str) -> DocError {
    DocError::InvalidSelector { selector: selector.to_string(), reason: reason.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeKind, RectGeometry};
    use crate::text::TextGeometry;

    fn rect(id: &'static str) -> Node {
        Node::new(
            NodeId::from_static(id),
            NodeKind::Rect(RectGeometry { width: 10.0, height: 10.0, corner_radius: [0.0; 4] }),
        )
    }

    /// root
    ///  └ section (@section, name "Features")
    ///     ├ card-a (@card)
    ///     │   └ title (text, @card-title)
    ///     └ card-b (@card @featured)
    fn doc() -> Document {
        let mut d = Document::new("Test");
        let title = Node::new(
            NodeId::from_static("nd_title"),
            NodeKind::Text(TextGeometry::new("Hi", "Inter", 16.0)),
        )
        .with_role("card-title")
        .with_name("Title");

        let card_a = rect("nd_card_a").with_role("card").with_children(vec![title]);
        let card_b = rect("nd_card_b").with_role("card").with_role("featured");

        let section = Node::new(NodeId::from_static("nd_section"), NodeKind::Group)
            .with_role("section")
            .with_name("Features")
            .with_children(vec![card_a, card_b]);

        d.pages[0].root.id = NodeId::from_static("nd_root");
        d.pages[0].root.children.push(section);
        d
    }

    fn select(s: &str) -> Vec<String> {
        Selector::parse(s)
            .unwrap()
            .select(&doc())
            .into_iter()
            .map(|i| i.as_str().to_string())
            .collect()
    }

    #[test]
    fn by_id() {
        assert_eq!(select("#nd_card_a"), vec!["nd_card_a"]);
    }

    #[test]
    fn by_role_is_the_reusable_case() {
        assert_eq!(select("@card"), vec!["nd_card_a", "nd_card_b"]);
    }

    #[test]
    fn by_type() {
        assert_eq!(select("type:text"), vec!["nd_title"]);
    }

    #[test]
    fn by_quoted_name() {
        assert_eq!(select("name:\"Features\""), vec!["nd_section"]);
    }

    #[test]
    fn compound_terms_must_all_hold() {
        assert_eq!(select("@card@featured"), vec!["nd_card_b"]);
        assert!(select("@card@nonexistent").is_empty());
    }

    #[test]
    fn descendant_combinator_looks_through_intermediate_levels() {
        assert_eq!(select("@section type:text"), vec!["nd_title"]);
        assert!(select("@card-title type:text").is_empty());
    }

    #[test]
    fn union_keeps_document_order_and_deduplicates() {
        assert_eq!(select("@card, @featured"), vec!["nd_card_a", "nd_card_b"]);
    }

    #[test]
    fn wildcard_matches_everything() {
        assert_eq!(select("*").len(), doc().node_count());
    }

    #[test]
    fn results_are_in_document_order() {
        assert_eq!(select("@featured, @card"), vec!["nd_card_a", "nd_card_b"]);
    }

    #[test]
    fn malformed_selectors_are_rejected_with_a_reason() {
        for bad in ["", "@", "#", "type:", "!!", "@card,", "name:\"unterminated"] {
            assert!(Selector::parse(bad).is_err(), "'{bad}' should not parse");
        }
    }

    #[test]
    fn missing_pages_are_an_error_not_an_empty_result() {
        let d = doc();
        let s = Selector::parse("@card").unwrap();
        assert!(s.select_in_page(&d, "index").is_ok());
        assert!(s.select_in_page(&d, "nope").is_err());
    }
}
