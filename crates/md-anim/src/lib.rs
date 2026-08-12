//! # md-anim
//!
//! Animation packages: the format, the registry that finds them, and the baker that
//! turns one into keyframes.
//!
//! The design goal is that the library keeps growing without the app growing with it.
//! Adding an animation is adding a folder containing a `manifest.json`. The manifest
//! declares its own parameters, and those parameter specs are what the studio's
//! inspector builds controls from — so a new animation arrives with a working UI and no
//! editor code was written for it.
//!
//! ```text
//! stagger-fade-up/
//!   manifest.json   id, version, params, and how params become keyframes
//!   preview.svg     thumbnail for the picker
//!   README.md       notes for humans and for future AI sessions
//! ```
//!
//! Manifests are data rather than code, which is what lets `md-cli`, the exporter and
//! the MCP server bake animations with no JavaScript runtime anywhere in sight. See
//! [`manifest`] for the reasoning.

pub mod bake;
pub mod error;
pub mod expr;
pub mod manifest;
pub mod registry;

pub use bake::{bake, BakeRequest};
pub use error::{AnimError, Result};
pub use manifest::{
    AnimationManifest, AppliesTo, Category, DeclarativeGenerator, Expr, Generator, ParamKind,
    ParamSpec, PerTarget, SelectOption, TrackTemplate,
};
pub use registry::{AnimationPackage, Origin, Registry};

/// Apply a package by id, resolving targets with a selector.
///
/// The convenience path most callers want: the MCP `anim.apply` tool, the CLI, and the
/// studio's animation picker all end up here.
pub fn apply_to_selector(
    doc: &md_doc::Document,
    registry: &Registry,
    package_id: &str,
    selector: &str,
    params: serde_json::Map<String, serde_json::Value>,
    page_key: Option<&str>,
) -> Result<md_doc::Timeline> {
    let pkg = registry.require(package_id)?;
    let sel = md_doc::Selector::parse(selector)?;

    let targets = match page_key {
        Some(key) => sel.select_in_page(doc, key)?,
        None => sel.select(doc),
    };

    if targets.is_empty() {
        return Err(AnimError::TargetMismatch {
            id: package_id.to_string(),
            reason: format!("'{selector}' matched no nodes"),
        });
    }

    bake(
        doc,
        &BakeRequest {
            manifest: &pkg.manifest,
            targets: &targets,
            selector,
            params,
            trigger: None,
            timeline_id: None,
            name: None,
        },
    )
}

/// Re-bake a timeline from the source it recorded.
///
/// This is what runs when a parameter changes in the inspector, and what a document
/// upgrade would run after installing a newer version of a package. It re-resolves the
/// selector, so a timeline picks up nodes added since it was first applied.
pub fn rebake(
    doc: &md_doc::Document,
    registry: &Registry,
    timeline: &md_doc::Timeline,
    page_key: Option<&str>,
) -> Result<md_doc::Timeline> {
    let source = timeline.source.as_ref().ok_or_else(|| AnimError::Bake {
        id: timeline.id.to_string(),
        reason: "this timeline was hand-authored and has no package to re-bake from".into(),
    })?;

    let pkg = registry.require(&source.package)?;
    let sel = md_doc::Selector::parse(&source.target)?;
    let targets = match page_key {
        Some(key) => sel.select_in_page(doc, key)?,
        None => sel.select(doc),
    };

    bake(
        doc,
        &BakeRequest {
            manifest: &pkg.manifest,
            targets: &targets,
            selector: &source.target,
            params: source.params.clone(),
            trigger: Some(timeline.trigger.clone()),
            // Keep the id so this replaces the timeline rather than adding another.
            timeline_id: Some(timeline.id.clone()),
            name: Some(timeline.name.clone()),
        },
    )
}
