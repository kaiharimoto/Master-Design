//! # md-doc
//!
//! The Master Design document model.
//!
//! ## The idea this crate exists to protect
//!
//! Master Design has two editors of equal standing: a person dragging handles, and an
//! AI model applying patches. Most "AI builds your site" tools make the model a code
//! generator — it emits output, the human edits that output, and the two immediately
//! desynchronize because the model can no longer read back what the human did without
//! re-parsing its own generated code.
//!
//! Here the *document* is the shared artifact, and neither editor is privileged:
//!
//! * Both mutate it through the same [`patch`] operations.
//! * Both get the same validation.
//! * Both land in the same [`history`] stack, so an AI edit is undoable with the same
//!   keystroke as a mouse drag.
//! * Both write the same bytes, because [`canonical`] serialization has exactly one
//!   spelling per state.
//!
//! ## Layout
//!
//! * [`node`], [`text`], [`paint`], [`transform`] — what a design is made of
//! * [`anim`] — timelines, tracks and easing
//! * [`document`] — projects, pages, lookup
//! * [`patch`] / [`history`] — how anything changes, and how it un-changes
//! * [`query`] — selectors, so a patch can address "every card" rather than an id
//! * [`digest`] — a compact rendering of a document for a model to read
//! * [`storage`] — the on-disk project directory

pub mod anim;
pub mod canonical;
pub mod digest;
pub mod document;
pub mod error;
pub mod history;
pub mod id;
pub mod node;
pub mod paint;
pub mod patch;
pub mod query;
pub mod storage;
pub mod text;
pub mod transform;

pub use anim::{AnimSource, Easing, Keyframe, Timeline, Track, Trigger};
pub use canonical::to_canonical_string;
pub use document::{Document, NodeLocation, Page, ProjectMeta, Tokens, SCHEMA_VERSION};
pub use error::{DocError, Result};
pub use history::{History, Transaction};
pub use id::{NodeId, PageId, TimelineId};
pub use node::{
    A11y, Constraints, EllipseGeometry, FrameGeometry, ImageGeometry, Node, NodeKind, PathGeometry,
    RectGeometry,
};
pub use paint::{BlendMode, Color, Effect, GradientStop, Paint, Stroke};
pub use patch::{Op, PatchReport};
pub use query::Selector;
pub use text::{TextAlign, TextGeometry};
pub use transform::Transform;
